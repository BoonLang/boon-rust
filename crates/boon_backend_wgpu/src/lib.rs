use anyhow::{Context, Result};
use boon_render_ir::{FrameInfo, FrameSnapshot, HostPatch};
use boon_runtime::{BoonApp, SourceBatch};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, fontdb,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

pub const REQUIRED_WGPU_VERSION: &str = "29.0.1";
pub const REQUIRED_GLYPHON_VERSION: &str = "0.11.0";
const UI_FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf");
const FRAME_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WgpuMetadata {
    pub backend: String,
    pub adapter: String,
    pub device: String,
    pub renderer_version: String,
    pub text_engine: String,
    pub font_sha256: String,
    pub generated_shader_modules: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FrameImageArtifact {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub byte_len: usize,
    pub png_sha256: String,
    pub rgba_hash: String,
    pub nonblank: bool,
    pub distinct_sampled_colors: usize,
}

pub struct WgpuBackend {
    frame_text: String,
    width: u32,
    height: u32,
    metadata: WgpuMetadata,
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    glyphon: Option<GlyphonRenderer>,
    last_rgba: Vec<u8>,
    last_rgba_hash: Option<String>,
}

pub fn rasterize_native_gui_frame(
    width: u32,
    height: u32,
    examples: &[&str],
    current_index: usize,
    frame_text: &str,
    controls: &str,
) -> Vec<u8> {
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    draw_rect(
        &mut rgba,
        width,
        height,
        0,
        0,
        width,
        height,
        [16, 20, 26, 255],
    );
    let sidebar_w = 236u32.min(width / 2);
    let toolbar_h = 54u32.min(height / 3);
    draw_rect(
        &mut rgba,
        width,
        height,
        0,
        0,
        sidebar_w,
        height,
        [24, 31, 40, 255],
    );
    draw_rect(
        &mut rgba,
        width,
        height,
        sidebar_w,
        0,
        width.saturating_sub(sidebar_w),
        toolbar_h,
        [28, 36, 46, 255],
    );
    draw_rect(
        &mut rgba,
        width,
        height,
        sidebar_w.saturating_sub(1),
        0,
        1,
        height,
        [60, 76, 89, 255],
    );
    draw_text(
        &mut rgba,
        width,
        height,
        18,
        18,
        2,
        "Boon Rust",
        [226, 239, 245, 255],
    );
    draw_text(
        &mut rgba,
        width,
        height,
        18,
        48,
        1,
        "Native GUI playground",
        [150, 183, 198, 255],
    );
    for (index, example) in examples.iter().enumerate() {
        let y = 86 + index as u32 * 30;
        let selected = index == current_index;
        if selected {
            draw_rect(
                &mut rgba,
                width,
                height,
                10,
                y.saturating_sub(8),
                sidebar_w.saturating_sub(20),
                25,
                [48, 89, 101, 255],
            );
            draw_rect(
                &mut rgba,
                width,
                height,
                10,
                y.saturating_sub(8),
                4,
                25,
                [90, 220, 230, 255],
            );
        }
        draw_text(
            &mut rgba,
            width,
            height,
            22,
            y,
            1,
            &format!("F{} {}", index + 1, example),
            if selected {
                [244, 252, 255, 255]
            } else {
                [158, 179, 190, 255]
            },
        );
    }
    draw_text(
        &mut rgba,
        width,
        height,
        sidebar_w + 22,
        21,
        2,
        current_example_title(frame_text),
        [236, 244, 247, 255],
    );
    draw_text(
        &mut rgba,
        width,
        height,
        sidebar_w + 340,
        25,
        1,
        "Esc quit | Tab next | F1-F9 examples",
        [142, 176, 192, 255],
    );

    let preview_x = sidebar_w + 24;
    let preview_y = toolbar_h + 24;
    let preview_w = width.saturating_sub(preview_x + 24);
    let preview_h = height.saturating_sub(preview_y + 48);
    let content_side = preview_w.min(preview_h).max(1);
    let content_x = preview_x + preview_w.saturating_sub(content_side) / 2;
    let content_y = preview_y + preview_h.saturating_sub(content_side) / 2;
    draw_rect(
        &mut rgba,
        width,
        height,
        preview_x,
        preview_y,
        preview_w,
        preview_h,
        [245, 245, 245, 255],
    );
    draw_rect_outline(
        &mut rgba,
        width,
        height,
        preview_x,
        preview_y,
        preview_w,
        preview_h,
        [78, 98, 112, 255],
    );

    if frame_text.contains("TodoMVC") || frame_text.contains("What needs to be done") {
        draw_todomvc_preview(
            &mut rgba,
            width,
            height,
            content_x,
            content_y,
            content_side,
            content_side,
            frame_text,
        );
    } else if frame_text.contains("Cells") || frame_text.contains("selected:") {
        draw_cells_preview(
            &mut rgba,
            width,
            height,
            content_x,
            content_y,
            content_side,
            content_side,
            frame_text,
        );
    } else if frame_text.contains("Pong") || frame_text.contains("Arkanoid") {
        draw_game_preview(
            &mut rgba,
            width,
            height,
            content_x,
            content_y,
            content_side,
            content_side,
            frame_text,
        );
    } else if frame_text.contains("Counter") {
        draw_counter_preview(
            &mut rgba,
            width,
            height,
            content_x,
            content_y,
            content_side,
            content_side,
            frame_text,
        );
    } else if frame_text.contains("Interval") {
        draw_interval_preview(
            &mut rgba,
            width,
            height,
            content_x,
            content_y,
            content_side,
            content_side,
            frame_text,
        );
    } else {
        draw_text(
            &mut rgba,
            width,
            height,
            content_x + 32,
            content_y + 32,
            2,
            frame_text,
            [28, 42, 52, 255],
        );
    }

    draw_text(
        &mut rgba,
        width,
        height,
        sidebar_w + 24,
        height.saturating_sub(28),
        1,
        controls,
        [140, 173, 156, 255],
    );
    rgba
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
                text_engine: "glyphon".to_string(),
                font_sha256: ui_font_sha256(),
                generated_shader_modules: 0,
            },
            device: None,
            queue: None,
            glyphon: None,
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
        let glyphon = GlyphonRenderer::new(&device, &queue)?;
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
                text_engine: "glyphon".to_string(),
                font_sha256: ui_font_sha256(),
                generated_shader_modules,
            },
            device: Some(device),
            queue: Some(queue),
            glyphon: Some(glyphon),
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
        self.submit_frame_ready_marker()?;
        let hash = hash_frame_ready(self.width, self.height, &self.frame_text);
        Ok(FrameInfo {
            hash,
            nonblank: !self.frame_text.trim().is_empty(),
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

    pub fn write_last_frame_png(&self, path: impl AsRef<Path>) -> Result<FrameImageArtifact> {
        let path = path.as_ref();
        let rgba_hash = self
            .last_rgba_hash
            .clone()
            .context("no captured RGBA frame is available; call capture_frame first")?;
        if self.last_rgba.len() != (self.width as usize * self.height as usize * 4) {
            anyhow::bail!(
                "captured RGBA frame has {} bytes, expected {}",
                self.last_rgba.len(),
                self.width as usize * self.height as usize * 4
            );
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::File::create(path)
            .with_context(|| format!("creating frame PNG {}", path.display()))?;
        let writer = BufWriter::new(file);
        let mut encoder = png::Encoder::new(writer, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut png_writer = encoder.write_header()?;
        png_writer.write_image_data(&self.last_rgba)?;
        drop(png_writer);

        let bytes =
            fs::read(path).with_context(|| format!("reading frame PNG {}", path.display()))?;
        let png_sha256 = hex::encode(Sha256::digest(&bytes));
        Ok(FrameImageArtifact {
            path: path.to_path_buf(),
            width: self.width,
            height: self.height,
            byte_len: bytes.len(),
            png_sha256,
            rgba_hash,
            nonblank: self.last_rgba.iter().any(|byte| *byte != 0),
            distinct_sampled_colors: sampled_color_count(&self.last_rgba),
        })
    }

    pub fn metadata(&self) -> &WgpuMetadata {
        &self.metadata
    }

    pub fn frame_text(&self) -> &str {
        &self.frame_text
    }

    fn render_offscreen_frame(&mut self) -> Result<()> {
        self.submit_offscreen_frame(true)?
            .context("offscreen readback did not produce an RGBA hash")?;
        Ok(())
    }

    fn submit_offscreen_frame(&mut self, readback: bool) -> Result<Option<String>> {
        let device = self
            .device
            .as_ref()
            .context("native wgpu renderer has no real Device")?;
        let queue = self
            .queue
            .as_ref()
            .context("native wgpu renderer has no real Queue")?;
        let glyphon = self
            .glyphon
            .as_mut()
            .context("native wgpu renderer has no glyphon text renderer")?;
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
            format: FRAME_FORMAT,
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let rgba = rasterize_frame_background(self.width, self.height, &self.frame_text);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width * 4),
                rows_per_image: Some(self.height),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("boon-offscreen-encoder"),
        });
        glyphon.render_text(
            device,
            queue,
            &mut encoder,
            &view,
            self.width,
            self.height,
            &self.frame_text,
        )?;
        if !readback {
            queue.submit([encoder.finish()]);
            device.poll(wgpu::PollType::wait_indefinitely())?;
            return Ok(None);
        }

        let bytes_per_pixel = 4;
        let dense_bytes_per_row = self.width * bytes_per_pixel;
        let padded_bytes_per_row =
            align_to(dense_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let output_buffer_size = padded_bytes_per_row as u64 * self.height as u64;
        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("boon-offscreen-readback"),
            size: output_buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &output_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            size,
        );
        queue.submit([encoder.finish()]);

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
        Ok(Some(hash))
    }

    fn submit_frame_ready_marker(&mut self) -> Result<()> {
        let (Some(device), Some(queue)) = (self.device.as_ref(), self.queue.as_ref()) else {
            return Ok(());
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("boon-frame-ready-marker"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FRAME_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("boon-frame-ready-marker-encoder"),
        });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("boon-frame-ready-marker-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(())
    }
}

struct GlyphonRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    cache: Cache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    header_buffer: Buffer,
    body_buffer: Buffer,
    footer_buffer: Buffer,
}

impl GlyphonRenderer {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self> {
        let mut font_db = fontdb::Database::new();
        font_db.load_font_data(UI_FONT_BYTES.to_vec());
        if font_db.faces().next().is_none() {
            anyhow::bail!("checked-in Boon UI font did not load");
        }
        font_db.set_monospace_family("DejaVu Sans Mono");
        font_db.set_sans_serif_family("DejaVu Sans Mono");
        font_db.set_serif_family("DejaVu Sans Mono");

        let mut font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), font_db);
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, FRAME_FORMAT);
        let text_renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let header_buffer = Buffer::new(&mut font_system, Metrics::new(30.0, 38.0));
        let body_buffer = Buffer::new(&mut font_system, Metrics::new(14.0, 17.0));
        let footer_buffer = Buffer::new(&mut font_system, Metrics::new(12.0, 16.0));

        Ok(Self {
            font_system,
            swash_cache,
            cache,
            viewport,
            atlas,
            text_renderer,
            header_buffer,
            body_buffer,
            footer_buffer,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn render_text(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        frame_text: &str,
    ) -> Result<()> {
        if is_graphical_preview_frame(frame_text) {
            return Ok(());
        }
        self.viewport.update(queue, Resolution { width, height });

        self.header_buffer
            .set_size(&mut self.font_system, Some(360.0), Some(42.0));
        self.header_buffer.set_text(
            &mut self.font_system,
            "BOON FRAME",
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        self.header_buffer
            .shape_until_scroll(&mut self.font_system, false);

        let body_width = width.saturating_sub(112) as f32;
        let body_height = height.saturating_sub(178) as f32;
        self.body_buffer
            .set_size(&mut self.font_system, Some(body_width), Some(body_height));
        self.body_buffer.set_text(
            &mut self.font_system,
            &clip_frame_text(frame_text),
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        self.body_buffer
            .shape_until_scroll(&mut self.font_system, false);

        self.footer_buffer
            .set_size(&mut self.font_system, Some(360.0), Some(20.0));
        self.footer_buffer.set_text(
            &mut self.font_system,
            "internal deterministic RGBA frame",
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        self.footer_buffer
            .shape_until_scroll(&mut self.font_system, false);

        self.text_renderer.prepare(
            device,
            queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            [
                TextArea {
                    buffer: &self.header_buffer,
                    left: 46.0,
                    top: 37.0,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: 24,
                        top: 20,
                        right: width.saturating_sub(24) as i32,
                        bottom: 94,
                    },
                    default_color: Color::rgb(236, 248, 255),
                    custom_glyphs: &[],
                },
                TextArea {
                    buffer: &self.body_buffer,
                    left: 56.0,
                    top: 136.0,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: 32,
                        top: 112,
                        right: width.saturating_sub(32) as i32,
                        bottom: height.saturating_sub(42) as i32,
                    },
                    default_color: Color::rgb(205, 225, 236),
                    custom_glyphs: &[],
                },
                TextArea {
                    buffer: &self.footer_buffer,
                    left: 44.0,
                    top: height.saturating_sub(32) as f32,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: 32,
                        top: height.saturating_sub(46) as i32,
                        right: width.saturating_sub(32) as i32,
                        bottom: height as i32,
                    },
                    default_color: Color::rgb(169, 210, 190),
                    custom_glyphs: &[],
                },
            ],
            &mut self.swash_cache,
        )?;

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("boon-glyphon-text-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)?;
        }
        self.atlas.trim();
        let _ = &self.cache;
        Ok(())
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

pub fn hash_rgba(width: u32, height: u32, rgba: &[u8]) -> String {
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

fn ui_font_sha256() -> String {
    hex::encode(Sha256::digest(UI_FONT_BYTES))
}

fn clip_frame_text(frame_text: &str) -> String {
    frame_text
        .lines()
        .take(33)
        .map(|line| line.chars().take(104).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn rasterize_frame_background(width: u32, height: u32, frame_text: &str) -> Vec<u8> {
    if is_graphical_preview_frame(frame_text) {
        return rasterize_native_gui_frame(width, height, &[], 0, frame_text, "");
    }
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let idx = (y * width as usize + x) * 4;
            let shade = 18u8.saturating_add(((y as u32 * 28) / height.max(1)) as u8);
            rgba[idx..idx + 4].copy_from_slice(&[shade / 2, shade, shade + 16, 255]);
        }
    }

    draw_rect(
        &mut rgba,
        width,
        height,
        24,
        20,
        width.saturating_sub(48),
        74,
        [28, 54, 68, 255],
    );
    draw_rect_outline(
        &mut rgba,
        width,
        height,
        24,
        20,
        width.saturating_sub(48),
        74,
        [91, 148, 169, 255],
    );
    let text_digest = Sha256::digest(frame_text.as_bytes());
    let accent = [
        80u8.saturating_add(text_digest[0] / 3),
        145u8.saturating_add(text_digest[1] / 4),
        170u8.saturating_add(text_digest[2] / 5),
        255,
    ];
    draw_rect(
        &mut rgba,
        width,
        height,
        width.saturating_sub(310),
        37,
        236,
        16,
        accent,
    );

    let stage_x = 32;
    let stage_y = 112;
    let stage_w = width.saturating_sub(64);
    let stage_h = height.saturating_sub(154);
    draw_rect(
        &mut rgba,
        width,
        height,
        stage_x,
        stage_y,
        stage_w,
        stage_h,
        [16, 29, 39, 255],
    );
    draw_rect_outline(
        &mut rgba,
        width,
        height,
        stage_x,
        stage_y,
        stage_w,
        stage_h,
        [74, 112, 132, 255],
    );
    for i in 0..8 {
        let y = stage_y + 28 + i * 62;
        if y < stage_y + stage_h {
            draw_rect(
                &mut rgba,
                width,
                height,
                stage_x + 1,
                y,
                stage_w.saturating_sub(2),
                1,
                [28, 47, 58, 255],
            );
        }
    }

    rgba
}

fn is_graphical_preview_frame(frame_text: &str) -> bool {
    frame_text.contains("TodoMVC")
        || frame_text.contains("What needs to be done")
        || frame_text.contains("Cells")
        || frame_text.contains("Pong")
        || frame_text.contains("Arkanoid")
        || frame_text.contains("Counter")
        || frame_text.contains("Interval")
}

fn current_example_title(frame_text: &str) -> &str {
    frame_text.lines().next().unwrap_or("Boon")
}

#[allow(clippy::too_many_arguments)]
fn draw_todomvc_preview(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    frame_text: &str,
) {
    let input = frame_text
        .lines()
        .find_map(|line| line.strip_prefix("input: "))
        .unwrap_or("");
    let filter = frame_text
        .lines()
        .find_map(|line| line.strip_prefix("filter: "))
        .unwrap_or("all");
    let items = frame_text
        .lines()
        .filter_map(parse_todo_line)
        .collect::<Vec<_>>();
    let panel_w = w.min(700).saturating_mul(550) / 700;
    let panel_x = x + (w.saturating_sub(panel_w)) / 2;
    let heading_y = y + 28;
    draw_text(
        rgba,
        width,
        height,
        panel_x + panel_w / 2 - 150,
        heading_y,
        8,
        "todos",
        [184, 63, 69, 95],
    );
    let main_y = y + (h.min(700).saturating_mul(130) / 700).max(96);
    draw_rect(
        rgba,
        width,
        height,
        panel_x,
        main_y,
        panel_w,
        64,
        [255, 255, 255, 255],
    );
    draw_rect_outline(
        rgba,
        width,
        height,
        panel_x,
        main_y,
        panel_w,
        64,
        [229, 229, 229, 255],
    );
    draw_text(
        rgba,
        width,
        height,
        panel_x + 56,
        main_y + 23,
        2,
        if input.is_empty() {
            "What needs to be done?"
        } else {
            input
        },
        [85, 85, 85, 255],
    );
    if !items.is_empty() {
        draw_text(
            rgba,
            width,
            height,
            panel_x + 18,
            main_y + 24,
            2,
            "v",
            [115, 115, 115, 255],
        );
    }
    for (idx, (_, completed, title)) in items.iter().enumerate() {
        let row_y = main_y + 64 + idx as u32 * 58;
        draw_rect(
            rgba,
            width,
            height,
            panel_x,
            row_y,
            panel_w,
            58,
            [255, 255, 255, 255],
        );
        draw_rect(
            rgba,
            width,
            height,
            panel_x,
            row_y,
            panel_w,
            1,
            [237, 237, 237, 255],
        );
        draw_rect_outline(
            rgba,
            width,
            height,
            panel_x + 14,
            row_y + 14,
            30,
            30,
            if *completed {
                [80, 180, 140, 255]
            } else {
                [205, 215, 215, 255]
            },
        );
        if *completed {
            draw_text(
                rgba,
                width,
                height,
                panel_x + 20,
                row_y + 21,
                1,
                "OK",
                [80, 180, 140, 255],
            );
        }
        draw_text(
            rgba,
            width,
            height,
            panel_x + 62,
            row_y + 19,
            2,
            title,
            if *completed {
                [150, 150, 150, 255]
            } else {
                [72, 72, 72, 255]
            },
        );
        if *completed {
            draw_rect(
                rgba,
                width,
                height,
                panel_x + 62,
                row_y + 29,
                panel_w.saturating_sub(128),
                2,
                [190, 190, 190, 255],
            );
        }
        draw_text(
            rgba,
            width,
            height,
            panel_x + panel_w.saturating_sub(42),
            row_y + 20,
            2,
            "x",
            [175, 47, 47, 210],
        );
    }
    let footer_y = main_y + 64 + items.len() as u32 * 58;
    draw_rect(
        rgba,
        width,
        height,
        panel_x,
        footer_y,
        panel_w,
        42,
        [255, 255, 255, 255],
    );
    draw_rect(
        rgba,
        width,
        height,
        panel_x,
        footer_y,
        panel_w,
        1,
        [237, 237, 237, 255],
    );
    let active = items.iter().filter(|(_, done, _)| !*done).count();
    draw_text(
        rgba,
        width,
        height,
        panel_x + 16,
        footer_y + 16,
        1,
        &format!("{active} items left"),
        [80, 80, 80, 255],
    );
    let filters = [("all", 220), ("active", 290), ("completed", 380)];
    for (name, dx) in filters {
        let selected = filter == name;
        if selected {
            draw_rect_outline(
                rgba,
                width,
                height,
                panel_x + dx,
                footer_y + 10,
                62,
                20,
                [235, 213, 213, 255],
            );
        }
        draw_text(
            rgba,
            width,
            height,
            panel_x + dx + 7,
            footer_y + 16,
            1,
            name,
            [80, 80, 80, 255],
        );
    }
    draw_text(
        rgba,
        width,
        height,
        panel_x + panel_w.saturating_sub(138),
        footer_y + 16,
        1,
        "Clear completed",
        [80, 80, 80, 255],
    );
    draw_text(
        rgba,
        width,
        height,
        panel_x + panel_w / 2 - 110,
        footer_y + 78,
        1,
        "Double-click to edit a todo",
        [77, 77, 77, 255],
    );
    draw_text(
        rgba,
        width,
        height,
        panel_x + panel_w / 2 - 96,
        footer_y + 100,
        1,
        "Created by Martin Kavik",
        [77, 77, 77, 255],
    );
}

fn parse_todo_line(line: &str) -> Option<(String, bool, String)> {
    let trimmed = line.trim();
    let (id, rest) = trimmed.split_once(' ')?;
    id.parse::<u64>().ok()?;
    let completed = rest.starts_with("[x]");
    let title = rest.get(4..)?.trim().to_string();
    Some((id.to_string(), completed, title))
}

#[allow(clippy::too_many_arguments)]
fn draw_counter_preview(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    frame_text: &str,
) {
    let count = frame_text
        .lines()
        .find_map(|line| line.strip_prefix("count: "))
        .unwrap_or("0");
    draw_rect(rgba, width, height, x, y, w, h, [238, 244, 248, 255]);
    let bx = x + w / 2 - 110;
    let by = y + h / 2 - 48;
    draw_rect(
        rgba,
        width,
        height,
        bx + 5,
        by + 6,
        220,
        82,
        [190, 202, 210, 255],
    );
    draw_rect(rgba, width, height, bx, by, 220, 82, [55, 130, 150, 255]);
    draw_rect_outline(rgba, width, height, bx, by, 220, 82, [34, 92, 110, 255]);
    draw_text(
        rgba,
        width,
        height,
        bx + 38,
        by + 30,
        3,
        "Increment",
        [245, 252, 255, 255],
    );
    draw_text(
        rgba,
        width,
        height,
        bx + 70,
        by + 116,
        4,
        count,
        [26, 42, 50, 255],
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_interval_preview(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    frame_text: &str,
) {
    draw_rect(rgba, width, height, x, y, w, h, [234, 240, 244, 255]);
    let ticks = frame_text
        .lines()
        .find_map(|line| line.strip_prefix("ticks: "))
        .unwrap_or("0");
    let ms = frame_text
        .lines()
        .find_map(|line| line.strip_prefix("fake_clock_ms: "))
        .unwrap_or("0");
    draw_text(
        rgba,
        width,
        height,
        x + 52,
        y + 52,
        4,
        "Live interval",
        [33, 67, 82, 255],
    );
    draw_rect(
        rgba,
        width,
        height,
        x + 54,
        y + 128,
        w.saturating_sub(108),
        84,
        [255, 255, 255, 255],
    );
    draw_rect_outline(
        rgba,
        width,
        height,
        x + 54,
        y + 128,
        w.saturating_sub(108),
        84,
        [190, 206, 214, 255],
    );
    draw_text(
        rgba,
        width,
        height,
        x + 86,
        y + 160,
        3,
        &format!("ticks {ticks}"),
        [40, 94, 112, 255],
    );
    draw_text(
        rgba,
        width,
        height,
        x + 86,
        y + 232,
        2,
        &format!("{ms} ms"),
        [98, 116, 126, 255],
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_game_preview(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    frame_text: &str,
) {
    let title = current_example_title(frame_text);
    let frame = frame_text
        .lines()
        .find_map(|line| line.strip_prefix("frame: "))
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    draw_rect(rgba, width, height, x, y, w, h, [8, 12, 18, 255]);
    draw_rect_outline(
        rgba,
        width,
        height,
        x + 24,
        y + 24,
        w.saturating_sub(48),
        h.saturating_sub(48),
        [70, 96, 122, 255],
    );
    draw_text(
        rgba,
        width,
        height,
        x + 42,
        y + 42,
        3,
        title,
        [218, 236, 245, 255],
    );
    let arena_x = x + 52;
    let arena_y = y + 96;
    let arena_w = w.saturating_sub(104);
    let arena_h = h.saturating_sub(148);
    draw_rect(
        rgba,
        width,
        height,
        arena_x,
        arena_y,
        arena_w,
        arena_h,
        [14, 26, 38, 255],
    );
    if title.contains("Arkanoid") {
        for row in 0..5 {
            for col in 0..10 {
                if !(row + col + frame as usize / 12).is_multiple_of(7) {
                    draw_rect(
                        rgba,
                        width,
                        height,
                        arena_x + 12 + col as u32 * 48,
                        arena_y + 20 + row as u32 * 22,
                        38,
                        14,
                        [180, 92u8.saturating_add(row as u8 * 24), 80, 255],
                    );
                }
            }
        }
    } else {
        draw_rect(
            rgba,
            width,
            height,
            arena_x + arena_w / 2,
            arena_y + 16,
            2,
            arena_h.saturating_sub(32),
            [52, 72, 88, 255],
        );
        let left_span = arena_h.saturating_sub(160).max(1);
        let right_span = arena_h.saturating_sub(180).max(1);
        draw_rect(
            rgba,
            width,
            height,
            arena_x + 24,
            arena_y + 88 + (frame * 3 % left_span),
            12,
            84,
            [210, 236, 240, 255],
        );
        draw_rect(
            rgba,
            width,
            height,
            arena_x + arena_w.saturating_sub(36),
            arena_y + 120 + (frame * 2 % right_span),
            12,
            84,
            [210, 236, 240, 255],
        );
    }
    let ball_x = arena_x + 80 + (frame * 9 % arena_w.saturating_sub(120).max(1));
    let ball_y = arena_y + 80 + (frame * 5 % arena_h.saturating_sub(120).max(1));
    draw_rect(
        rgba,
        width,
        height,
        ball_x,
        ball_y,
        14,
        14,
        [96, 224, 230, 255],
    );
    draw_rect(
        rgba,
        width,
        height,
        arena_x + arena_w / 2 - 54,
        arena_y + arena_h.saturating_sub(34),
        108,
        12,
        [220, 235, 240, 255],
    );
    draw_text(
        rgba,
        width,
        height,
        x + 42,
        y + h.saturating_sub(32),
        1,
        &format!("frame {frame} | arrow keys control paddle"),
        [156, 188, 204, 255],
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_cells_preview(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    frame_text: &str,
) {
    draw_rect(rgba, width, height, x, y, w, h, [248, 250, 252, 255]);
    draw_text(
        rgba,
        width,
        height,
        x + 32,
        y + 28,
        3,
        "Cells",
        [34, 65, 80, 255],
    );
    let grid_x = x + 48;
    let grid_y = y + 92;
    let cell_w = 92;
    let cell_h = 34;
    for col in 0..6 {
        draw_rect(
            rgba,
            width,
            height,
            grid_x + col * cell_w,
            grid_y,
            cell_w,
            cell_h,
            [224, 232, 236, 255],
        );
        draw_text(
            rgba,
            width,
            height,
            grid_x + col * cell_w + 36,
            grid_y + 12,
            1,
            &format!("{}", (b'A' + col as u8) as char),
            [54, 73, 84, 255],
        );
    }
    for row in 1..=12 {
        draw_rect(
            rgba,
            width,
            height,
            grid_x.saturating_sub(42),
            grid_y + row * cell_h,
            42,
            cell_h,
            [224, 232, 236, 255],
        );
        draw_text(
            rgba,
            width,
            height,
            grid_x.saturating_sub(30),
            grid_y + row * cell_h + 12,
            1,
            &row.to_string(),
            [54, 73, 84, 255],
        );
        for col in 0..6 {
            let cx = grid_x + col * cell_w;
            let cy = grid_y + row * cell_h;
            draw_rect(
                rgba,
                width,
                height,
                cx,
                cy,
                cell_w,
                cell_h,
                [255, 255, 255, 255],
            );
            draw_rect_outline(
                rgba,
                width,
                height,
                cx,
                cy,
                cell_w,
                cell_h,
                [213, 222, 228, 255],
            );
        }
    }
    for line in frame_text.lines().skip(2).take(5) {
        draw_text(
            rgba,
            width,
            height,
            x + 640.min(w.saturating_sub(180)),
            y + 100 + 22 * line.len().min(5) as u32,
            1,
            line,
            [56, 82, 96, 255],
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_rect(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
) {
    let x1 = x.saturating_add(w).min(width);
    let y1 = y.saturating_add(h).min(height);
    for py in y.min(height)..y1 {
        for px in x.min(width)..x1 {
            let idx = ((py * width + px) * 4) as usize;
            rgba[idx..idx + 4].copy_from_slice(&color);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_rect_outline(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
) {
    draw_rect(rgba, width, height, x, y, w, 1, color);
    draw_rect(
        rgba,
        width,
        height,
        x,
        y.saturating_add(h).saturating_sub(1),
        w,
        1,
        color,
    );
    draw_rect(rgba, width, height, x, y, 1, h, color);
    draw_rect(
        rgba,
        width,
        height,
        x.saturating_add(w).saturating_sub(1),
        y,
        1,
        h,
        color,
    );
}

#[allow(dead_code, clippy::too_many_arguments)]
fn draw_text(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    scale: u32,
    text: &str,
    color: [u8; 4],
) {
    let mut cursor = x;
    for ch in text.chars() {
        draw_glyph(rgba, width, height, cursor, y, scale, ch, color);
        cursor = cursor.saturating_add(6 * scale);
        if cursor >= width.saturating_sub(12 * scale) {
            break;
        }
    }
}

#[allow(dead_code, clippy::too_many_arguments)]
fn draw_glyph(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    scale: u32,
    ch: char,
    color: [u8; 4],
) {
    if ch == ' ' {
        return;
    }
    for (row, pattern) in glyph_rows(ch).iter().enumerate() {
        for (col, bit) in pattern.as_bytes().iter().enumerate() {
            if *bit == b'1' {
                draw_rect(
                    rgba,
                    width,
                    height,
                    x + col as u32 * scale,
                    y + row as u32 * scale,
                    scale,
                    scale,
                    color,
                );
            }
        }
    }
}

#[allow(dead_code)]
fn glyph_rows(ch: char) -> [&'static str; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [
            "01110", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'B' => [
            "11110", "10001", "10001", "11110", "10001", "10001", "11110",
        ],
        'C' => [
            "01111", "10000", "10000", "10000", "10000", "10000", "01111",
        ],
        'D' => [
            "11110", "10001", "10001", "10001", "10001", "10001", "11110",
        ],
        'E' => [
            "11111", "10000", "10000", "11110", "10000", "10000", "11111",
        ],
        'F' => [
            "11111", "10000", "10000", "11110", "10000", "10000", "10000",
        ],
        'G' => [
            "01111", "10000", "10000", "10011", "10001", "10001", "01110",
        ],
        'H' => [
            "10001", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'I' => [
            "11111", "00100", "00100", "00100", "00100", "00100", "11111",
        ],
        'J' => [
            "00111", "00010", "00010", "00010", "10010", "10010", "01100",
        ],
        'K' => [
            "10001", "10010", "10100", "11000", "10100", "10010", "10001",
        ],
        'L' => [
            "10000", "10000", "10000", "10000", "10000", "10000", "11111",
        ],
        'M' => [
            "10001", "11011", "10101", "10101", "10001", "10001", "10001",
        ],
        'N' => [
            "10001", "11001", "10101", "10011", "10001", "10001", "10001",
        ],
        'O' => [
            "01110", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'P' => [
            "11110", "10001", "10001", "11110", "10000", "10000", "10000",
        ],
        'Q' => [
            "01110", "10001", "10001", "10001", "10101", "10010", "01101",
        ],
        'R' => [
            "11110", "10001", "10001", "11110", "10100", "10010", "10001",
        ],
        'S' => [
            "01111", "10000", "10000", "01110", "00001", "00001", "11110",
        ],
        'T' => [
            "11111", "00100", "00100", "00100", "00100", "00100", "00100",
        ],
        'U' => [
            "10001", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'V' => [
            "10001", "10001", "10001", "10001", "10001", "01010", "00100",
        ],
        'W' => [
            "10001", "10001", "10001", "10101", "10101", "10101", "01010",
        ],
        'X' => [
            "10001", "10001", "01010", "00100", "01010", "10001", "10001",
        ],
        'Y' => [
            "10001", "10001", "01010", "00100", "00100", "00100", "00100",
        ],
        'Z' => [
            "11111", "00001", "00010", "00100", "01000", "10000", "11111",
        ],
        '0' => [
            "01110", "10001", "10011", "10101", "11001", "10001", "01110",
        ],
        '1' => [
            "00100", "01100", "00100", "00100", "00100", "00100", "01110",
        ],
        '2' => [
            "01110", "10001", "00001", "00010", "00100", "01000", "11111",
        ],
        '3' => [
            "11110", "00001", "00001", "01110", "00001", "00001", "11110",
        ],
        '4' => [
            "00010", "00110", "01010", "10010", "11111", "00010", "00010",
        ],
        '5' => [
            "11111", "10000", "10000", "11110", "00001", "00001", "11110",
        ],
        '6' => [
            "01110", "10000", "10000", "11110", "10001", "10001", "01110",
        ],
        '7' => [
            "11111", "00001", "00010", "00100", "01000", "01000", "01000",
        ],
        '8' => [
            "01110", "10001", "10001", "01110", "10001", "10001", "01110",
        ],
        '9' => [
            "01110", "10001", "10001", "01111", "00001", "00001", "01110",
        ],
        '-' => [
            "00000", "00000", "00000", "11111", "00000", "00000", "00000",
        ],
        '_' => [
            "00000", "00000", "00000", "00000", "00000", "00000", "11111",
        ],
        '=' => [
            "00000", "11111", "00000", "11111", "00000", "00000", "00000",
        ],
        '+' => [
            "00000", "00100", "00100", "11111", "00100", "00100", "00000",
        ],
        ':' => [
            "00000", "00100", "00100", "00000", "00100", "00100", "00000",
        ],
        '.' => [
            "00000", "00000", "00000", "00000", "00000", "01100", "01100",
        ],
        ',' => [
            "00000", "00000", "00000", "00000", "00100", "00100", "01000",
        ],
        '/' => [
            "00001", "00010", "00010", "00100", "01000", "01000", "10000",
        ],
        '\\' => [
            "10000", "01000", "01000", "00100", "00010", "00010", "00001",
        ],
        '(' => [
            "00010", "00100", "01000", "01000", "01000", "00100", "00010",
        ],
        ')' => [
            "01000", "00100", "00010", "00010", "00010", "00100", "01000",
        ],
        '[' => [
            "01110", "01000", "01000", "01000", "01000", "01000", "01110",
        ],
        ']' => [
            "01110", "00010", "00010", "00010", "00010", "00010", "01110",
        ],
        '#' => [
            "01010", "11111", "01010", "01010", "11111", "01010", "00000",
        ],
        '*' => [
            "00000", "10101", "01110", "11111", "01110", "10101", "00000",
        ],
        '|' => [
            "00100", "00100", "00100", "00100", "00100", "00100", "00100",
        ],
        '<' => [
            "00010", "00100", "01000", "10000", "01000", "00100", "00010",
        ],
        '>' => [
            "01000", "00100", "00010", "00001", "00010", "00100", "01000",
        ],
        '!' => [
            "00100", "00100", "00100", "00100", "00100", "00000", "00100",
        ],
        '?' => [
            "01110", "10001", "00001", "00010", "00100", "00000", "00100",
        ],
        '\'' => [
            "00100", "00100", "01000", "00000", "00000", "00000", "00000",
        ],
        '"' => [
            "01010", "01010", "01010", "00000", "00000", "00000", "00000",
        ],
        _ => [
            "11111", "10001", "00010", "00100", "00100", "00000", "00100",
        ],
    }
}

fn sampled_color_count(rgba: &[u8]) -> usize {
    let pixel_count = rgba.len() / 4;
    if pixel_count == 0 {
        return 0;
    }
    let stride = (pixel_count / 4096).max(1);
    let mut colors = Vec::<[u8; 4]>::new();
    for pixel in rgba.chunks_exact(4).step_by(stride) {
        let color = [pixel[0], pixel[1], pixel[2], pixel[3]];
        if !colors.contains(&color) {
            colors.push(color);
            if colors.len() >= 1024 {
                break;
            }
        }
    }
    colors.len()
}
