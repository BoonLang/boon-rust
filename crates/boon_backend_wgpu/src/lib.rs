use anyhow::{Context, Result};
use boon_render_ir::{FrameInfo, FrameSnapshot, HostPatch};
use boon_runtime::{BoonApp, SourceBatch};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;

pub const REQUIRED_WGPU_VERSION: &str = "29.0.1";
pub const REQUIRED_GLYPHON_VERSION: &str = "0.11.0";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WgpuMetadata {
    pub backend: String,
    pub adapter: String,
    pub device: String,
    pub renderer_version: String,
    pub generated_shader_modules: usize,
}

pub struct WgpuBackend {
    frame_text: String,
    width: u32,
    height: u32,
    metadata: WgpuMetadata,
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    last_rgba: Vec<u8>,
    last_rgba_hash: Option<String>,
}

impl WgpuBackend {
    pub fn headless(width: u32, height: u32) -> Self {
        Self {
            frame_text: String::new(),
            width,
            height,
            metadata: WgpuMetadata {
                backend: "wgpu-headless-framebuffer".to_string(),
                adapter: "not-yet-bound-to-real-adapter".to_string(),
                device: "software-verification-surface".to_string(),
                renderer_version: format!(
                    "wgpu-{REQUIRED_WGPU_VERSION}/glyphon-{REQUIRED_GLYPHON_VERSION}"
                ),
                generated_shader_modules: 0,
            },
            device: None,
            queue: None,
            last_rgba: Vec::new(),
            last_rgba_hash: None,
        }
    }

    pub fn headless_real(width: u32, height: u32) -> Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let info = adapter.get_info();
        let descriptor = wgpu::DeviceDescriptor {
            label: Some("boon-headless-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&descriptor))?;
        let generated_shader_modules = load_generated_shader_modules(&device)?;
        Ok(Self {
            frame_text: String::new(),
            width,
            height,
            metadata: WgpuMetadata {
                backend: format!("{:?}", info.backend),
                adapter: info.name,
                device: format!("{:?}", info.device_type),
                renderer_version: format!(
                    "wgpu-{REQUIRED_WGPU_VERSION}/glyphon-{REQUIRED_GLYPHON_VERSION}"
                ),
                generated_shader_modules,
            },
            device: Some(device),
            queue: Some(queue),
            last_rgba: Vec::new(),
            last_rgba_hash: None,
        })
    }

    pub fn load<A: BoonApp>(&mut self, app: &mut A) -> Result<FrameInfo> {
        let turn = app.mount();
        self.apply_patches(&turn.patches)?;
        self.render_frame()
    }

    pub fn dispatch<A: BoonApp>(&mut self, app: &mut A, batch: SourceBatch) -> Result<FrameInfo> {
        for result in app.dispatch_batch(batch)? {
            self.apply_patches(&result.patches)?;
        }
        self.render_frame()
    }

    pub fn dispatch_frame_ready<A: BoonApp>(
        &mut self,
        app: &mut A,
        batch: SourceBatch,
    ) -> Result<FrameInfo> {
        for result in app.dispatch_batch(batch)? {
            self.apply_patches(&result.patches)?;
        }
        self.render_frame_ready()
    }

    pub fn apply_patches(&mut self, patches: &[HostPatch]) -> Result<()> {
        for patch in patches {
            if let HostPatch::ReplaceFrameText { text } = patch {
                self.frame_text = text.clone();
            }
        }
        Ok(())
    }

    pub fn render_frame(&mut self) -> Result<FrameInfo> {
        self.render_offscreen_frame()?;
        let hash = self
            .last_rgba_hash
            .clone()
            .context("offscreen frame did not produce an RGBA hash")?;
        Ok(FrameInfo {
            hash,
            nonblank: self.last_rgba.iter().any(|byte| *byte != 0),
        })
    }

    pub fn render_frame_ready(&mut self) -> Result<FrameInfo> {
        let hash = self.submit_offscreen_clear(false)?;
        Ok(FrameInfo {
            hash,
            nonblank: true,
        })
    }

    pub fn capture_frame(&mut self) -> Result<FrameSnapshot> {
        self.render_offscreen_frame()?;
        Ok(FrameSnapshot {
            width: self.width,
            height: self.height,
            text: self.frame_text.clone(),
            rgba_hash: self.last_rgba_hash.clone(),
        })
    }

    pub fn metadata(&self) -> &WgpuMetadata {
        &self.metadata
    }

    fn render_offscreen_frame(&mut self) -> Result<()> {
        self.submit_offscreen_clear(true)?;
        Ok(())
    }

    fn submit_offscreen_clear(&mut self, readback: bool) -> Result<String> {
        let device = self
            .device
            .as_ref()
            .context("native wgpu renderer has no real Device")?;
        let queue = self
            .queue
            .as_ref()
            .context("native wgpu renderer has no real Queue")?;
        let size = wgpu::Extent3d {
            width: self.width,
            height: self.height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("boon-offscreen-frame"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: if readback {
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
            } else {
                wgpu::TextureUsages::RENDER_ATTACHMENT
            },
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let clear = self.clear_color();

        let bytes_per_pixel = 4;
        let dense_bytes_per_row = self.width * bytes_per_pixel;
        let padded_bytes_per_row =
            align_to(dense_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let output_buffer = if readback {
            let output_buffer_size = padded_bytes_per_row as u64 * self.height as u64;
            Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("boon-offscreen-readback"),
                size: output_buffer_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }))
        } else {
            None
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("boon-offscreen-encoder"),
        });
        {
            let color_attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("boon-offscreen-clear-pass"),
                color_attachments: &color_attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        if let Some(output_buffer) = output_buffer.as_ref() {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: output_buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_bytes_per_row),
                        rows_per_image: Some(self.height),
                    },
                },
                size,
            );
        }
        queue.submit([encoder.finish()]);

        let Some(output_buffer) = output_buffer else {
            device.poll(wgpu::PollType::Poll)?;
            return Ok(hash_frame_ready(self.width, self.height, &self.frame_text));
        };

        let slice = output_buffer.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        device.poll(wgpu::PollType::wait_indefinitely())?;
        receiver
            .recv()
            .context("wgpu readback callback did not run")?
            .context("wgpu readback map failed")?;

        let mapped = slice.get_mapped_range();
        let mut rgba = Vec::with_capacity((self.width * self.height * bytes_per_pixel) as usize);
        for row in 0..self.height as usize {
            let start = row * padded_bytes_per_row as usize;
            let end = start + dense_bytes_per_row as usize;
            rgba.extend_from_slice(&mapped[start..end]);
        }
        drop(mapped);
        output_buffer.unmap();

        let hash = hash_rgba(self.width, self.height, &rgba);
        self.last_rgba = rgba;
        self.last_rgba_hash = Some(hash.clone());
        Ok(hash)
    }

    fn clear_color(&self) -> wgpu::Color {
        let digest = Sha256::digest(self.frame_text.as_bytes());
        wgpu::Color {
            r: (digest[0] as f64 + 1.0) / 256.0,
            g: (digest[1] as f64 + 1.0) / 256.0,
            b: (digest[2] as f64 + 1.0) / 256.0,
            a: 1.0,
        }
    }
}

fn load_generated_shader_modules(device: &wgpu::Device) -> Result<usize> {
    let generated = generated_shader_dir()
        .context("missing generated shader directory; run `cargo xtask shaders` first")?;
    let roots = [
        "ui_rects.wgsl",
        "ui_text.wgsl",
        "grid.wgsl",
        "physical_debug.wgsl",
        "present.wgsl",
    ];
    for root in roots {
        let path = generated.join(root);
        let source = fs::read_to_string(&path).with_context(|| {
            format!(
                "missing generated WGSL {}; run `cargo xtask shaders`",
                path.display()
            )
        })?;
        let _module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(root),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
    }
    Ok(roots.len())
}

fn generated_shader_dir() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("target/generated-shaders");
        if candidate.join("bindings.rs").exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

fn hash_rgba(width: u32, height: u32, rgba: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(width.to_le_bytes());
    hasher.update(height.to_le_bytes());
    hasher.update(rgba);
    hex::encode(hasher.finalize())
}

fn hash_frame_ready(width: u32, height: u32, frame_text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(width.to_le_bytes());
    hasher.update(height.to_le_bytes());
    hasher.update(frame_text.as_bytes());
    hex::encode(hasher.finalize())
}
