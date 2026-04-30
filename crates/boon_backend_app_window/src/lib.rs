use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppWindowSmoke {
    pub logical_width: f64,
    pub logical_height: f64,
    pub scale: f64,
    pub wgpu_backend: String,
    pub wgpu_adapter: String,
    pub surface_format: String,
}

pub fn smoke_test() -> Result<AppWindowSmoke> {
    let result = Arc::new(Mutex::new(None));
    let result_for_closure = Arc::clone(&result);
    app_window::test_support::integration_test_harness(move || {
        let smoke = pollster::block_on(surface_smoke()).map_err(|err| err.to_string());
        *result_for_closure.lock().expect("smoke result lock") = Some(smoke);
    });

    let smoke = result
        .lock()
        .expect("smoke result lock")
        .take()
        .context("app_window smoke did not return a result")?;
    smoke.map_err(anyhow::Error::msg)
}

async fn surface_smoke() -> Result<AppWindowSmoke> {
    use app_window::coordinates::{Position, Size};
    use app_window::window::Window;

    let mut window = Window::new(
        Position::new(16.0, 16.0),
        Size::new(320.0, 200.0),
        "Boon app_window smoke".to_string(),
    )
    .await;
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
    let (device, _queue) = adapter
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

    Ok(AppWindowSmoke {
        logical_width: size.width(),
        logical_height: size.height(),
        scale,
        wgpu_backend: format!("{:?}", adapter_info.backend),
        wgpu_adapter: adapter_info.name,
        surface_format,
    })
}
