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
use std::sync::mpsc;
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppWindowSurfaceFrameProof {
    pub configured_width: u32,
    pub configured_height: u32,
    pub final_surface_width: u32,
    pub final_surface_height: u32,
    pub rgba_hash: String,
    pub byte_len: usize,
    pub distinct_sampled_colors: usize,
    pub dominant_rgba: [u8; 4],
    pub dominant_ratio: f64,
    pub nonblank: bool,
    pub size_matches_final_surface: bool,
    pub passed: bool,
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

pub fn run_rgba_input_session<S, F, G>(
    title: impl Into<String>,
    hold: Duration,
    tick: Duration,
    state: S,
    on_input: F,
    frame_rgba: G,
) -> Result<(AppWindowSmoke, S)>
where
    S: Send + 'static,
    F: FnMut(&mut S, AppWindowInputSample) -> Result<()> + Send + 'static,
    G: FnMut(&mut S, u32, u32) -> Result<RgbaFrame> + Send + 'static,
{
    let result = Arc::new(Mutex::new(None));
    let result_for_closure = Arc::clone(&result);
    let title = title.into();
    app_window::test_support::integration_test_harness(move || {
        let session = pollster::block_on(rgba_input_session(
            title, hold, tick, state, on_input, frame_rgba, false,
        ))
        .map(|(smoke, state, _)| (smoke, state))
        .map_err(|err| err.to_string());
        *result_for_closure
            .lock()
            .expect("rgba input session result lock") = Some(session);
    });

    let session = result
        .lock()
        .expect("rgba input session result lock")
        .take()
        .context("app_window rgba input session did not return a result")?;
    session.map_err(anyhow::Error::msg)
}

pub fn run_rgba_input_session_with_proof<S, F, G>(
    title: impl Into<String>,
    hold: Duration,
    tick: Duration,
    state: S,
    on_input: F,
    frame_rgba: G,
) -> Result<(AppWindowSmoke, S, Option<AppWindowSurfaceFrameProof>)>
where
    S: Send + 'static,
    F: FnMut(&mut S, AppWindowInputSample) -> Result<()> + Send + 'static,
    G: FnMut(&mut S, u32, u32) -> Result<RgbaFrame> + Send + 'static,
{
    let result = Arc::new(Mutex::new(None));
    let result_for_closure = Arc::clone(&result);
    let title = title.into();
    app_window::test_support::integration_test_harness(move || {
        let session = pollster::block_on(rgba_input_session(
            title, hold, tick, state, on_input, frame_rgba, true,
        ))
        .map_err(|err| err.to_string());
        *result_for_closure
            .lock()
            .expect("rgba input session result lock") = Some(session);
    });

    let session = result
        .lock()
        .expect("rgba input session result lock")
        .take()
        .context("app_window rgba input session did not return a result")?;
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

async fn rgba_input_session<S, F, G>(
    title: String,
    hold: Duration,
    tick: Duration,
    state: S,
    mut on_input: F,
    mut frame_rgba: G,
    capture_surface_proof: bool,
) -> Result<(AppWindowSmoke, S, Option<AppWindowSurfaceFrameProof>)>
where
    S: Send + 'static,
    F: FnMut(&mut S, AppWindowInputSample) -> Result<()> + Send + 'static,
    G: FnMut(&mut S, u32, u32) -> Result<RgbaFrame> + Send + 'static,
{
    use app_window::coordinates::{Position, Size};
    use app_window::input::keyboard::Keyboard;
    use app_window::input::keyboard::key::KeyboardKey;
    use app_window::input::mouse::{MOUSE_BUTTON_LEFT, Mouse};
    use app_window::window::Window;

    let mut window = Window::new(Position::new(16.0, 16.0), Size::new(1120.0, 760.0), title).await;
    let surface = window.surface().await;
    let (initial_size, mut scale) = surface.size_scale().await;
    let mut size = initial_size;
    if size.width() <= 0.0 || size.height() <= 0.0 {
        bail!("app_window created a non-positive surface: {size:?} scale {scale}");
    }
    let mut width = size.width() as u32;
    let mut height = size.height() as u32;

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
            label: Some("boon-app-window-rgba-playground-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        })
        .await?;
    let mut config = wgpu_surface
        .get_default_config(&adapter, width, height)
        .context("app_window wgpu surface did not provide a default config")?;
    config.usage |= wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC;
    let surface_format = format!("{:?}", config.format);
    wgpu_surface.configure(&device, &config);

    let keyboard = Keyboard::coalesced().await;
    let mut mouse = Mouse::coalesced().await;
    let keys = KeyboardKey::all_keys();
    let mut previous_keys = BTreeSet::<String>::new();
    let mut previous_left = false;
    let mut state = state;
    let mut surface_proofs = Vec::new();
    let started = Instant::now();
    let deadline = started + hold;
    loop {
        let (current_size, current_scale) = surface.size_scale().await;
        let current_width = current_size.width() as u32;
        let current_height = current_size.height() as u32;
        if current_width > 0
            && current_height > 0
            && (current_width != width || current_height != height)
        {
            size = current_size;
            scale = current_scale;
            width = current_width;
            height = current_height;
            config.width = width;
            config.height = height;
            wgpu_surface.configure(&device, &config);
        }
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

        let frame = frame_rgba(&mut state, width, height)?;
        if capture_surface_proof {
            surface_proofs.push(present_rgba_frame(
                &device,
                &queue,
                &wgpu_surface,
                config.format,
                frame,
            )?);
        } else {
            present_rgba_frame_fast(&queue, &wgpu_surface, config.format, frame)?;
        }
        if hold.is_zero() || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(tick);
    }

    let (final_size, _) = surface.size_scale().await;
    let mut last_surface_proof = surface_proofs.pop();
    if let Some(proof) = &mut last_surface_proof {
        proof.final_surface_width = final_size.width() as u32;
        proof.final_surface_height = final_size.height() as u32;
        proof.size_matches_final_surface = proof.configured_width == proof.final_surface_width
            && proof.configured_height == proof.final_surface_height;
        proof.passed = proof.nonblank
            && proof.distinct_sampled_colors >= 8
            && proof.size_matches_final_surface;
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
        last_surface_proof,
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

fn present_rgba_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface: &wgpu::Surface<'_>,
    format: wgpu::TextureFormat,
    frame: RgbaFrame,
) -> Result<AppWindowSurfaceFrameProof> {
    if frame.rgba.len() != frame.width as usize * frame.height as usize * 4 {
        bail!(
            "RGBA frame has {} bytes, expected {}",
            frame.rgba.len(),
            frame.width as usize * frame.height as usize * 4
        );
    }
    let frame_texture = match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame)
        | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
        other => bail!("app_window surface did not provide a presentable texture: {other:?}"),
    };
    let pixels = surface_format_pixels(format, &frame.rgba)?;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &frame_texture.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(frame.width * 4),
            rows_per_image: Some(frame.height),
        },
        wgpu::Extent3d {
            width: frame.width,
            height: frame.height,
            depth_or_array_layers: 1,
        },
    );
    let proof = readback_surface_frame(
        device,
        queue,
        &frame_texture.texture,
        format,
        frame.width,
        frame.height,
    )?;
    frame_texture.present();
    Ok(proof)
}

fn present_rgba_frame_fast(
    queue: &wgpu::Queue,
    surface: &wgpu::Surface<'_>,
    format: wgpu::TextureFormat,
    frame: RgbaFrame,
) -> Result<()> {
    if frame.rgba.len() != frame.width as usize * frame.height as usize * 4 {
        bail!(
            "RGBA frame has {} bytes, expected {}",
            frame.rgba.len(),
            frame.width as usize * frame.height as usize * 4
        );
    }
    let frame_texture = match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame)
        | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
        other => bail!("app_window surface did not provide a presentable texture: {other:?}"),
    };
    let pixels = surface_format_pixels(format, &frame.rgba)?;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &frame_texture.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(frame.width * 4),
            rows_per_image: Some(frame.height),
        },
        wgpu::Extent3d {
            width: frame.width,
            height: frame.height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::empty());
    frame_texture.present();
    Ok(())
}

fn readback_surface_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> Result<AppWindowSurfaceFrameProof> {
    let bytes_per_pixel = 4;
    let dense_bytes_per_row = width * bytes_per_pixel;
    let padded_bytes_per_row = align_to(dense_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let output_buffer_size = padded_bytes_per_row as u64 * height as u64;
    let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("boon-app-window-surface-readback"),
        size: output_buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("boon-app-window-surface-readback-encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &output_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
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
        .context("app_window surface readback callback did not run")?
        .context("app_window surface readback map failed")?;
    let mapped = slice.get_mapped_range();
    let mut surface_pixels = Vec::with_capacity((width * height * bytes_per_pixel) as usize);
    for row in 0..height as usize {
        let start = row * padded_bytes_per_row as usize;
        let end = start + dense_bytes_per_row as usize;
        surface_pixels.extend_from_slice(&mapped[start..end]);
    }
    drop(mapped);
    output_buffer.unmap();
    let rgba = surface_pixels_to_rgba(format, &surface_pixels)?;
    let (distinct_sampled_colors, dominant_rgba, dominant_ratio) = surface_color_stats(&rgba);
    let nonblank = distinct_sampled_colors > 1 && dominant_ratio < 0.995;
    Ok(AppWindowSurfaceFrameProof {
        configured_width: width,
        configured_height: height,
        final_surface_width: width,
        final_surface_height: height,
        rgba_hash: boon_backend_wgpu::hash_rgba(width, height, &rgba),
        byte_len: rgba.len(),
        distinct_sampled_colors,
        dominant_rgba,
        dominant_ratio,
        nonblank,
        size_matches_final_surface: true,
        passed: nonblank && distinct_sampled_colors >= 8,
    })
}

fn surface_format_pixels(format: wgpu::TextureFormat, rgba: &[u8]) -> Result<Vec<u8>> {
    match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => Ok(rgba.to_vec()),
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => {
            let mut bgra = rgba.to_vec();
            for pixel in bgra.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            Ok(bgra)
        }
        other => bail!("unsupported app_window surface format for RGBA playground: {other:?}"),
    }
}

fn surface_pixels_to_rgba(format: wgpu::TextureFormat, pixels: &[u8]) -> Result<Vec<u8>> {
    match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => {
            Ok(pixels.to_vec())
        }
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => {
            let mut rgba = pixels.to_vec();
            for pixel in rgba.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            Ok(rgba)
        }
        other => bail!("unsupported app_window surface format for RGBA readback: {other:?}"),
    }
}

fn surface_color_stats(rgba: &[u8]) -> (usize, [u8; 4], f64) {
    let pixel_count = rgba.len() / 4;
    if pixel_count == 0 {
        return (0, [0, 0, 0, 0], 1.0);
    }
    let stride = (pixel_count / 8192).max(1);
    let mut colors = Vec::<([u8; 4], usize)>::new();
    let mut samples = 0usize;
    for pixel in rgba.chunks_exact(4).step_by(stride) {
        samples += 1;
        let color = [pixel[0], pixel[1], pixel[2], pixel[3]];
        if let Some((_, count)) = colors.iter_mut().find(|(candidate, _)| *candidate == color) {
            *count += 1;
        } else if colors.len() < 2048 {
            colors.push((color, 1));
        }
    }
    colors.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let (dominant_rgba, dominant_count) = colors.first().copied().unwrap_or(([0, 0, 0, 0], 0));
    (
        colors.len(),
        dominant_rgba,
        dominant_count as f64 / samples.max(1) as f64,
    )
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
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
