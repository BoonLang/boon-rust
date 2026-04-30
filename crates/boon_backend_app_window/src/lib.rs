use anyhow::{Context, Result, bail};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, fontdb,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const UI_FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppWindowSmoke {
    pub logical_width: f64,
    pub logical_height: f64,
    pub scale: f64,
    pub wgpu_backend: String,
    pub wgpu_adapter: String,
    pub surface_format: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppWindowInputSample {
    pub elapsed_ms: u128,
    pub mouse_x: Option<f64>,
    pub mouse_y: Option<f64>,
    pub mouse_window_width: Option<f64>,
    pub mouse_window_height: Option<f64>,
    pub left_pressed: bool,
    pub left_clicked: bool,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub pressed_keys: Vec<String>,
    pub newly_pressed_keys: Vec<String>,
}

pub fn smoke_test() -> Result<AppWindowSmoke> {
    smoke_test_with_title("Boon app_window smoke", Duration::ZERO)
}

pub fn smoke_test_with_title(title: impl Into<String>, hold: Duration) -> Result<AppWindowSmoke> {
    smoke_test_via_helper(title.into(), hold)
}

pub fn smoke_test_with_title_direct(
    title: impl Into<String>,
    hold: Duration,
) -> Result<AppWindowSmoke> {
    let result = Arc::new(Mutex::new(None));
    let result_for_closure = Arc::clone(&result);
    let title = title.into();
    app_window::test_support::integration_test_harness(move || {
        let smoke = pollster::block_on(surface_smoke(title, hold)).map_err(|err| err.to_string());
        *result_for_closure.lock().expect("smoke result lock") = Some(smoke);
    });

    let smoke = result
        .lock()
        .expect("smoke result lock")
        .take()
        .context("app_window smoke did not return a result")?;
    smoke.map_err(anyhow::Error::msg)
}

pub fn smoke_test_helper_main(title: String, hold: Duration, out: PathBuf) -> ! {
    app_window::test_support::integration_test_harness(move || {
        let result = pollster::block_on(surface_smoke(title, hold));
        let status = if let Err(err) = write_smoke_helper_result(&out, result) {
            eprintln!(
                "failed to write app_window smoke result {}: {err}",
                out.display()
            );
            1
        } else {
            0
        };
        std::process::exit(status);
    });
    std::process::exit(2)
}

fn smoke_test_via_helper(title: String, hold: Duration) -> Result<AppWindowSmoke> {
    let out = std::env::temp_dir().join(format!(
        "boon-app-window-smoke-{}-{}.json",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let root = repo_root().context("finding repo root for app_window smoke helper")?;
    let mut child = Command::new("cargo")
        .current_dir(&root)
        .args([
            "run",
            "--quiet",
            "-p",
            "boon_backend_app_window",
            "--bin",
            "boon_app_window_smoke",
            "--",
            "--title",
            &title,
            "--hold-ms",
            &hold.as_millis().to_string(),
            "--out",
            &out.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning app_window smoke helper")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("app_window smoke helper timed out after 15s");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let bytes = fs::read(&out)
        .with_context(|| format!("reading app_window smoke helper output {}", out.display()))?;
    let _ = fs::remove_file(&out);
    let response: SmokeHelperResult = serde_json::from_slice(&bytes)?;
    if !status.success() {
        bail!(
            "app_window smoke helper exited with {status}: {}",
            response
                .error
                .unwrap_or_else(|| "unknown error".to_string())
        );
    }
    response
        .smoke
        .context("app_window smoke helper succeeded without smoke payload")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SmokeHelperResult {
    smoke: Option<AppWindowSmoke>,
    error: Option<String>,
}

fn write_smoke_helper_result(path: &Path, result: Result<AppWindowSmoke>) -> Result<()> {
    let response = match result {
        Ok(smoke) => SmokeHelperResult {
            smoke: Some(smoke),
            error: None,
        },
        Err(err) => SmokeHelperResult {
            smoke: None,
            error: Some(err.to_string()),
        },
    };
    fs::write(path, serde_json::to_vec_pretty(&response)?)?;
    Ok(())
}

fn repo_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join("IMPLEMENTATION_PLAN.md").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!("could not find repo root containing IMPLEMENTATION_PLAN.md");
        }
    }
}

pub fn run_input_session<S, F>(
    title: impl Into<String>,
    hold: Duration,
    tick: Duration,
    state: S,
    on_input: F,
) -> Result<(AppWindowSmoke, S)>
where
    S: Send + 'static,
    F: FnMut(&mut S, AppWindowInputSample) -> Result<()> + Send + 'static,
{
    let result = Arc::new(Mutex::new(None));
    let result_for_closure = Arc::clone(&result);
    let title = title.into();
    app_window::test_support::integration_test_harness(move || {
        let session = pollster::block_on(input_session(title, hold, tick, state, on_input))
            .map_err(|err| err.to_string());
        *result_for_closure
            .lock()
            .expect("input session result lock") = Some(session);
    });

    let session = result
        .lock()
        .expect("input session result lock")
        .take()
        .context("app_window input session did not return a result")?;
    session.map_err(anyhow::Error::msg)
}

pub fn run_text_input_session<S, F, G>(
    title: impl Into<String>,
    hold: Duration,
    tick: Duration,
    state: S,
    on_input: F,
    frame_text: G,
) -> Result<(AppWindowSmoke, S)>
where
    S: Send + 'static,
    F: FnMut(&mut S, AppWindowInputSample) -> Result<()> + Send + 'static,
    G: FnMut(&mut S) -> Result<String> + Send + 'static,
{
    let result = Arc::new(Mutex::new(None));
    let result_for_closure = Arc::clone(&result);
    let title = title.into();
    app_window::test_support::integration_test_harness(move || {
        let session = pollster::block_on(text_input_session(
            title, hold, tick, state, on_input, frame_text,
        ))
        .map_err(|err| err.to_string());
        *result_for_closure
            .lock()
            .expect("text input session result lock") = Some(session);
    });

    let session = result
        .lock()
        .expect("text input session result lock")
        .take()
        .context("app_window text input session did not return a result")?;
    session.map_err(anyhow::Error::msg)
}

async fn surface_smoke(title: String, hold: Duration) -> Result<AppWindowSmoke> {
    use app_window::coordinates::{Position, Size};
    use app_window::window::Window;

    let mut window = Window::new(Position::new(16.0, 16.0), Size::new(320.0, 200.0), title).await;
    let surface = window.surface().await;
    let (size, scale) = surface.size_scale().await;
    if size.width() <= 0.0 || size.height() <= 0.0 {
        bail!("app_window created a non-positive surface: {size:?} scale {scale}");
    }

    let instance = wgpu::Instance::default();
    let wgpu_surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(surface.raw_display_handle()),
            raw_window_handle: surface.raw_window_handle(),
        })?
    };
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: Some(&wgpu_surface),
        })
        .await?;
    let adapter_info = adapter.get_info();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("boon-app-window-smoke-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        })
        .await?;
    let config = wgpu_surface
        .get_default_config(&adapter, size.width() as u32, size.height() as u32)
        .context("app_window wgpu surface did not provide a default config")?;
    let surface_format = format!("{:?}", config.format);
    wgpu_surface.configure(&device, &config);
    present_smoke_frame(&device, &queue, &wgpu_surface)?;
    if !hold.is_zero() {
        std::thread::sleep(hold);
    }

    Ok(AppWindowSmoke {
        logical_width: size.width(),
        logical_height: size.height(),
        scale,
        wgpu_backend: format!("{:?}", adapter_info.backend),
        wgpu_adapter: adapter_info.name,
        surface_format,
    })
}

async fn input_session<S, F>(
    title: String,
    hold: Duration,
    tick: Duration,
    state: S,
    mut on_input: F,
) -> Result<(AppWindowSmoke, S)>
where
    S: Send + 'static,
    F: FnMut(&mut S, AppWindowInputSample) -> Result<()> + Send + 'static,
{
    use app_window::coordinates::{Position, Size};
    use app_window::input::keyboard::Keyboard;
    use app_window::input::keyboard::key::KeyboardKey;
    use app_window::input::mouse::{MOUSE_BUTTON_LEFT, Mouse};
    use app_window::window::Window;

    let mut window = Window::new(Position::new(16.0, 16.0), Size::new(960.0, 640.0), title).await;
    let surface = window.surface().await;
    let (size, scale) = surface.size_scale().await;
    if size.width() <= 0.0 || size.height() <= 0.0 {
        bail!("app_window created a non-positive surface: {size:?} scale {scale}");
    }

    let instance = wgpu::Instance::default();
    let wgpu_surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(surface.raw_display_handle()),
            raw_window_handle: surface.raw_window_handle(),
        })?
    };
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: Some(&wgpu_surface),
        })
        .await?;
    let adapter_info = adapter.get_info();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("boon-app-window-input-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        })
        .await?;
    let config = wgpu_surface
        .get_default_config(&adapter, size.width() as u32, size.height() as u32)
        .context("app_window wgpu surface did not provide a default config")?;
    let surface_format = format!("{:?}", config.format);
    wgpu_surface.configure(&device, &config);

    let keyboard = Keyboard::coalesced().await;
    let mut mouse = Mouse::coalesced().await;
    let keys = KeyboardKey::all_keys();
    let mut previous_keys = BTreeSet::<String>::new();
    let mut previous_left = false;
    let mut state = state;
    let started = Instant::now();
    let deadline = started + hold;
    loop {
        present_smoke_frame(&device, &queue, &wgpu_surface)?;
        let pressed_keys = keys
            .iter()
            .copied()
            .filter(|key| keyboard.is_pressed(*key))
            .map(|key| format!("{key:?}"))
            .collect::<BTreeSet<_>>();
        let newly_pressed_keys = pressed_keys
            .difference(&previous_keys)
            .cloned()
            .collect::<Vec<_>>();
        let left_pressed = mouse.button_state(MOUSE_BUTTON_LEFT);
        let left_clicked = left_pressed && !previous_left;
        let location = mouse.window_pos();
        let (scroll_x, scroll_y) = mouse.load_clear_scroll_delta();
        let sample = AppWindowInputSample {
            elapsed_ms: started.elapsed().as_millis(),
            mouse_x: location.map(|location| location.pos_x()),
            mouse_y: location.map(|location| location.pos_y()),
            mouse_window_width: location.map(|location| location.window_width()),
            mouse_window_height: location.map(|location| location.window_height()),
            left_pressed,
            left_clicked,
            scroll_x,
            scroll_y,
            pressed_keys: pressed_keys.iter().cloned().collect(),
            newly_pressed_keys,
        };
        on_input(&mut state, sample)?;
        previous_keys = pressed_keys;
        previous_left = left_pressed;
        if hold.is_zero() || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(tick);
    }

    Ok((
        AppWindowSmoke {
            logical_width: size.width(),
            logical_height: size.height(),
            scale,
            wgpu_backend: format!("{:?}", adapter_info.backend),
            wgpu_adapter: adapter_info.name,
            surface_format,
        },
        state,
    ))
}

async fn text_input_session<S, F, G>(
    title: String,
    hold: Duration,
    tick: Duration,
    state: S,
    mut on_input: F,
    mut frame_text: G,
) -> Result<(AppWindowSmoke, S)>
where
    S: Send + 'static,
    F: FnMut(&mut S, AppWindowInputSample) -> Result<()> + Send + 'static,
    G: FnMut(&mut S) -> Result<String> + Send + 'static,
{
    use app_window::coordinates::{Position, Size};
    use app_window::input::keyboard::Keyboard;
    use app_window::input::keyboard::key::KeyboardKey;
    use app_window::input::mouse::{MOUSE_BUTTON_LEFT, Mouse};
    use app_window::window::Window;

    let mut window = Window::new(Position::new(16.0, 16.0), Size::new(1120.0, 760.0), title).await;
    let surface = window.surface().await;
    let (size, scale) = surface.size_scale().await;
    if size.width() <= 0.0 || size.height() <= 0.0 {
        bail!("app_window created a non-positive surface: {size:?} scale {scale}");
    }

    let instance = wgpu::Instance::default();
    let wgpu_surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(surface.raw_display_handle()),
            raw_window_handle: surface.raw_window_handle(),
        })?
    };
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: Some(&wgpu_surface),
        })
        .await?;
    let adapter_info = adapter.get_info();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("boon-app-window-playground-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        })
        .await?;
    let config = wgpu_surface
        .get_default_config(&adapter, size.width() as u32, size.height() as u32)
        .context("app_window wgpu surface did not provide a default config")?;
    let surface_format = format!("{:?}", config.format);
    wgpu_surface.configure(&device, &config);
    let mut presenter = TextPresenter::new(&device, &queue, config.format)?;

    let keyboard = Keyboard::coalesced().await;
    let mut mouse = Mouse::coalesced().await;
    let keys = KeyboardKey::all_keys();
    let mut previous_keys = BTreeSet::<String>::new();
    let mut previous_left = false;
    let mut state = state;
    let started = Instant::now();
    let deadline = started + hold;
    loop {
        let text = frame_text(&mut state)?;
        present_text_frame(
            &device,
            &queue,
            &wgpu_surface,
            &mut presenter,
            size.width() as u32,
            size.height() as u32,
            &text,
        )?;
        let pressed_keys = keys
            .iter()
            .copied()
            .filter(|key| keyboard.is_pressed(*key))
            .map(|key| format!("{key:?}"))
            .collect::<BTreeSet<_>>();
        let newly_pressed_keys = pressed_keys
            .difference(&previous_keys)
            .cloned()
            .collect::<Vec<_>>();
        if newly_pressed_keys.iter().any(|key| key == "Escape") {
            break;
        }
        let left_pressed = mouse.button_state(MOUSE_BUTTON_LEFT);
        let left_clicked = left_pressed && !previous_left;
        let location = mouse.window_pos();
        let (scroll_x, scroll_y) = mouse.load_clear_scroll_delta();
        let sample = AppWindowInputSample {
            elapsed_ms: started.elapsed().as_millis(),
            mouse_x: location.map(|location| location.pos_x()),
            mouse_y: location.map(|location| location.pos_y()),
            mouse_window_width: location.map(|location| location.window_width()),
            mouse_window_height: location.map(|location| location.window_height()),
            left_pressed,
            left_clicked,
            scroll_x,
            scroll_y,
            pressed_keys: pressed_keys.iter().cloned().collect(),
            newly_pressed_keys,
        };
        on_input(&mut state, sample)?;
        previous_keys = pressed_keys;
        previous_left = left_pressed;
        if hold.is_zero() || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(tick);
    }

    Ok((
        AppWindowSmoke {
            logical_width: size.width(),
            logical_height: size.height(),
            scale,
            wgpu_backend: format!("{:?}", adapter_info.backend),
            wgpu_adapter: adapter_info.name,
            surface_format,
        },
        state,
    ))
}

fn present_smoke_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface: &wgpu::Surface<'_>,
) -> Result<()> {
    let frame = match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame)
        | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
        other => bail!("app_window surface did not provide a presentable texture: {other:?}"),
    };
    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("boon-app-window-smoke-present"),
    });
    {
        let color_attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: 0.04,
                    g: 0.09,
                    b: 0.16,
                    a: 1.0,
                }),
                store: wgpu::StoreOp::Store,
            },
        })];
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("boon-app-window-smoke-present-pass"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    queue.submit([encoder.finish()]);
    frame.present();
    Ok(())
}

struct TextPresenter {
    font_system: FontSystem,
    swash_cache: SwashCache,
    cache: Cache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    buffer: Buffer,
    footer: Buffer,
}

impl TextPresenter {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let mut font_db = fontdb::Database::new();
        font_db.load_font_data(UI_FONT_BYTES.to_vec());
        if font_db.faces().next().is_none() {
            bail!("checked-in Boon UI font did not load");
        }
        font_db.set_monospace_family("DejaVu Sans Mono");
        font_db.set_sans_serif_family("DejaVu Sans Mono");
        font_db.set_serif_family("DejaVu Sans Mono");
        let mut font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), font_db);
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let buffer = Buffer::new(&mut font_system, Metrics::new(15.0, 20.0));
        let footer = Buffer::new(&mut font_system, Metrics::new(13.0, 18.0));
        Ok(Self {
            font_system,
            swash_cache,
            cache,
            viewport,
            atlas,
            text_renderer,
            buffer,
            footer,
        })
    }

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        text: &str,
    ) -> Result<()> {
        self.viewport.update(queue, Resolution { width, height });
        let content_width = width.saturating_sub(48) as f32;
        let content_height = height.saturating_sub(84) as f32;
        self.buffer.set_size(
            &mut self.font_system,
            Some(content_width),
            Some(content_height),
        );
        self.buffer.set_text(
            &mut self.font_system,
            text,
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        self.buffer.shape_until_scroll(&mut self.font_system, false);

        self.footer.set_size(
            &mut self.font_system,
            Some(width.saturating_sub(48) as f32),
            Some(24.0),
        );
        self.footer.set_text(
            &mut self.font_system,
            "Esc quits | Tab/PageDown next example | PageUp previous | F1-F9 direct example",
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        self.footer.shape_until_scroll(&mut self.font_system, false);

        self.text_renderer.prepare(
            device,
            queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            [
                TextArea {
                    buffer: &self.buffer,
                    left: 24.0,
                    top: 24.0,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: 16,
                        top: 16,
                        right: width.saturating_sub(16) as i32,
                        bottom: height.saturating_sub(44) as i32,
                    },
                    default_color: Color::rgb(226, 239, 245),
                    custom_glyphs: &[],
                },
                TextArea {
                    buffer: &self.footer,
                    left: 24.0,
                    top: height.saturating_sub(34) as f32,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: 16,
                        top: height.saturating_sub(44) as i32,
                        right: width.saturating_sub(16) as i32,
                        bottom: height as i32,
                    },
                    default_color: Color::rgb(160, 205, 190),
                    custom_glyphs: &[],
                },
            ],
            &mut self.swash_cache,
        )?;
        Ok(())
    }
}

fn present_text_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface: &wgpu::Surface<'_>,
    presenter: &mut TextPresenter,
    width: u32,
    height: u32,
    text: &str,
) -> Result<()> {
    let frame = match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame)
        | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
        other => bail!("app_window surface did not provide a presentable texture: {other:?}"),
    };
    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    presenter.prepare(device, queue, width, height, text)?;
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("boon-app-window-playground-present"),
    });
    {
        let color_attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: 0.015,
                    g: 0.028,
                    b: 0.038,
                    a: 1.0,
                }),
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("boon-app-window-playground-present-pass"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        presenter
            .text_renderer
            .render(&presenter.atlas, &presenter.viewport, &mut pass)?;
    }
    presenter.atlas.trim();
    let _ = &presenter.cache;
    queue.submit([encoder.finish()]);
    frame.present();
    Ok(())
}
