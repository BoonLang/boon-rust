use anyhow::{Context, Result, bail};
use boon_backend_app_window::{
    AppWindowInputSample, run_text_input_session,
    smoke_test_with_title as app_window_smoke_test_with_title,
};
use boon_backend_browser::BrowserScenarioInput;
use boon_backend_ratatui::RatatuiBackend;
use boon_backend_wgpu::{FrameImageArtifact, WgpuBackend};
use boon_examples::{app, list_examples};
use boon_runtime::{BoonApp, SourceBatch, SourceEmission, SourceValue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufReader, BufWriter, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Backend {
    RatatuiBuffer,
    RatatuiPty,
    NativeWgpuHeadless,
    NativeAppWindow,
    BrowserFirefoxWgpu,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GateResult {
    pub backend: Backend,
    pub example: String,
    pub passed: bool,
    pub frame_hash: Option<String>,
    pub artifact_dir: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerifyReport {
    pub command: String,
    pub results: Vec<GateResult>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub example: String,
    pub steps: Vec<ScenarioStep>,
    pub assertions: Vec<ScenarioAssertion>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ScenarioStep {
    Mount,
    Click { target: String },
    Focus { target: String },
    Blur { target: String },
    TypeText { target: String, text: String },
    KeyDown { target: String, key: String },
    Change { target: String },
    AdvanceClock { millis: u64 },
    AdvanceFrame { target: String },
    Timing { name: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ScenarioAssertion {
    VisibleOutput,
    SemanticState { path: String },
    SourceInventory,
    SourceBinding { path: String },
    DeterministicReplay,
    FrameHash,
    TimingBudget { name: String },
    ErrorRejected { description: String },
}

impl Scenario {
    pub fn new(example: impl Into<String>) -> Self {
        Self {
            example: example.into(),
            steps: Vec::new(),
            assertions: Vec::new(),
        }
    }

    pub fn mount(mut self) -> Self {
        self.steps.push(ScenarioStep::Mount);
        self
    }

    pub fn click(mut self, target: impl Into<String>) -> Self {
        self.steps.push(ScenarioStep::Click {
            target: target.into(),
        });
        self
    }

    pub fn focus(mut self, target: impl Into<String>) -> Self {
        self.steps.push(ScenarioStep::Focus {
            target: target.into(),
        });
        self
    }

    pub fn blur(mut self, target: impl Into<String>) -> Self {
        self.steps.push(ScenarioStep::Blur {
            target: target.into(),
        });
        self
    }

    pub fn type_text(mut self, target: impl Into<String>, text: impl Into<String>) -> Self {
        self.steps.push(ScenarioStep::TypeText {
            target: target.into(),
            text: text.into(),
        });
        self
    }

    pub fn key_down(mut self, target: impl Into<String>, key: impl Into<String>) -> Self {
        self.steps.push(ScenarioStep::KeyDown {
            target: target.into(),
            key: key.into(),
        });
        self
    }

    pub fn change(mut self, target: impl Into<String>) -> Self {
        self.steps.push(ScenarioStep::Change {
            target: target.into(),
        });
        self
    }

    pub fn advance_clock(mut self, millis: u64) -> Self {
        self.steps.push(ScenarioStep::AdvanceClock { millis });
        self
    }

    pub fn advance_frame(mut self, target: impl Into<String>) -> Self {
        self.steps.push(ScenarioStep::AdvanceFrame {
            target: target.into(),
        });
        self
    }

    pub fn timing(mut self, name: impl Into<String>) -> Self {
        self.steps.push(ScenarioStep::Timing { name: name.into() });
        self
    }

    pub fn expect_visible_output(mut self) -> Self {
        self.assertions.push(ScenarioAssertion::VisibleOutput);
        self
    }

    pub fn expect_state(mut self, path: impl Into<String>) -> Self {
        self.assertions
            .push(ScenarioAssertion::SemanticState { path: path.into() });
        self
    }

    pub fn expect_source_inventory(mut self) -> Self {
        self.assertions.push(ScenarioAssertion::SourceInventory);
        self
    }

    pub fn expect_source_binding(mut self, path: impl Into<String>) -> Self {
        self.assertions
            .push(ScenarioAssertion::SourceBinding { path: path.into() });
        self
    }

    pub fn expect_replay(mut self) -> Self {
        self.assertions.push(ScenarioAssertion::DeterministicReplay);
        self
    }

    pub fn expect_frame_hash(mut self) -> Self {
        self.assertions.push(ScenarioAssertion::FrameHash);
        self
    }

    pub fn expect_timing_budget(mut self, name: impl Into<String>) -> Self {
        self.assertions
            .push(ScenarioAssertion::TimingBudget { name: name.into() });
        self
    }

    pub fn expect_error_rejected(mut self, description: impl Into<String>) -> Self {
        self.assertions.push(ScenarioAssertion::ErrorRejected {
            description: description.into(),
        });
        self
    }

    pub fn replay_steps(&self) -> Vec<String> {
        self.steps.iter().map(ScenarioStep::description).collect()
    }

    pub fn human_steps(&self) -> Vec<String> {
        self.steps
            .iter()
            .filter_map(ScenarioStep::human_description)
            .collect()
    }
}

impl ScenarioStep {
    fn description(&self) -> String {
        match self {
            ScenarioStep::Mount => "mount".to_string(),
            ScenarioStep::Click { target } => format!("click {target}"),
            ScenarioStep::Focus { target } => format!("focus {target}"),
            ScenarioStep::Blur { target } => format!("blur {target}"),
            ScenarioStep::TypeText { target, text } => {
                format!("type {target} text character-by-character ({text})")
            }
            ScenarioStep::KeyDown { target, key } => format!("key_down {target} {key}"),
            ScenarioStep::Change { target } => format!("emit change for {target}"),
            ScenarioStep::AdvanceClock { millis } => format!("advance_fake_time {millis}ms"),
            ScenarioStep::AdvanceFrame { target } => {
                format!("advance deterministic frame {target}")
            }
            ScenarioStep::Timing { name } => format!("timing scenario {name}"),
        }
    }

    fn human_description(&self) -> Option<String> {
        match self {
            ScenarioStep::Mount | ScenarioStep::Timing { .. } => None,
            step => Some(step.description()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ReplayProof {
    passed: bool,
    snapshot_hash: String,
    replay_snapshot_hash: String,
    frame_hash: Option<String>,
    replay_frame_hash: Option<String>,
    steps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ImageArtifactProof {
    path: PathBuf,
    width: u32,
    height: u32,
    byte_len: usize,
    png_sha256: String,
    sampled_colors: usize,
    dominant_rgba: [u8; 4],
    dominant_ratio: f64,
    nonblank: bool,
    not_error_solid: bool,
    passed: bool,
}

pub fn verify_ratatui(artifacts: &Path, pty: bool) -> Result<VerifyReport> {
    let backend = if pty {
        Backend::RatatuiPty
    } else {
        Backend::RatatuiBuffer
    };
    let mut results = Vec::new();
    for name in list_examples() {
        let mut app = app(name)?;
        let mut backend_impl = RatatuiBackend::new(120, 40);
        let info = backend_impl.load(&mut app)?;
        run_core_scenario(name, &mut app, &mut backend_impl)?;
        let timing = ratatui_timing_gate(name, &mut app, &mut backend_impl)?;
        let frame = backend_impl.capture_frame()?;
        let pty_capture = if pty {
            Some(capture_frame_through_pty(&frame.text)?)
        } else {
            None
        };
        let dir = artifacts
            .join(name)
            .join(if pty { "ratatui-pty" } else { "ratatui" });
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("frames.txt"), &frame.text)?;
        let frame_png = write_text_frame_png(&frame.text, &dir.join("frame.png"))?;
        if let Some(capture) = &pty_capture {
            fs::write(dir.join("pty-capture.txt"), capture)?;
        }
        fs::write(
            dir.join("timings.json"),
            serde_json::to_vec_pretty(&timing)?,
        )?;
        let scenario = scenario_for_example(name);
        fs::write(
            dir.join("trace.json"),
            serde_json::to_vec_pretty(&json!({
                "example": name,
                "mode": if pty { "pty" } else { "buffer" },
                "scenario_builder": &scenario,
                "initial_hash": info.hash,
                "final_hash": stable_sha(&frame.text),
                "pty_capture_hash": pty_capture.as_ref().map(|capture| stable_sha(capture)),
                "frame_png": &frame_png,
                "source_inventory": app.source_inventory(),
                "snapshot": app.snapshot(),
            }))?,
        )?;
        let replay = replay_ratatui(name, pty, &app.snapshot(), &stable_sha(&frame.text))?;
        fs::write(dir.join("replay.json"), serde_json::to_vec_pretty(&replay)?)?;
        let pty_matches = pty_capture
            .as_ref()
            .is_none_or(|capture| normalize_terminal_capture(capture).contains(frame.text.trim()));
        let timing_passed = timing
            .get("passed")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);
        results.push(GateResult {
            backend: backend.clone(),
            example: (*name).to_string(),
            passed: pty_matches && timing_passed && replay.passed && frame_png.passed,
            frame_hash: Some(stable_sha(&frame.text)),
            artifact_dir: dir,
            message: if pty_matches && timing_passed && replay.passed && frame_png.passed {
                "passed deterministic semantic/frame text, PNG frame artifact, and replay gate"
                    .to_string()
            } else if !timing_passed {
                "timing budget gate failed".to_string()
            } else if !replay.passed {
                "replay gate failed".to_string()
            } else if !frame_png.passed {
                "Ratatui frame PNG artifact check failed".to_string()
            } else {
                "PTY capture did not contain rendered Ratatui frame text".to_string()
            },
        });
    }
    Ok(VerifyReport {
        command: format!("verify ratatui{}", if pty { " --pty" } else { "" }),
        results,
    })
}

fn capture_frame_through_pty(frame_text: &str) -> Result<String> {
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system.openpty(portable_pty::PtySize {
        rows: 40,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let cmd = portable_pty::CommandBuilder::new("cat");
    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    writer.write_all(frame_text.as_bytes())?;
    writer.write_all(b"\n")?;
    drop(writer);
    let status = child.wait()?;
    let mut output = String::new();
    reader.read_to_string(&mut output)?;
    if !status.success() {
        bail!("PTY cat process exited with {status:?}");
    }
    Ok(output)
}

fn normalize_terminal_capture(capture: &str) -> String {
    capture.replace("\r\n", "\n").replace('\r', "\n")
}

fn write_wgpu_frame_png(
    backend: &WgpuBackend,
    dir: &Path,
    file_name: &str,
) -> Result<FrameImageArtifact> {
    let proof = backend.write_last_frame_png(dir.join(file_name))?;
    if !proof.nonblank
        || proof.distinct_sampled_colors <= 1
        || proof.byte_len == 0
        || proof.rgba_hash.is_empty()
    {
        bail!(
            "internal frame PNG {} failed basic image checks",
            proof.path.display()
        );
    }
    Ok(proof)
}

fn write_text_frame_png(frame_text: &str, path: &Path) -> Result<ImageArtifactProof> {
    let cell_w = 6usize;
    let cell_h = 10usize;
    let cols = 120usize;
    let rows = 40usize;
    let width = (cols * cell_w) as u32;
    let height = (rows * cell_h) as u32;
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[13, 24, 32, 255]);
    }
    for (row, line) in frame_text.lines().take(rows).enumerate() {
        for (col, ch) in line.chars().take(cols).enumerate() {
            if ch == ' ' {
                continue;
            }
            let digest = Sha256::digest(ch.to_string().as_bytes());
            let color = [
                120u8.saturating_add(digest[0] / 3),
                170u8.saturating_add(digest[1] / 4),
                190u8.saturating_add(digest[2] / 5),
                255,
            ];
            let x0 = col * cell_w + 1;
            let y0 = row * cell_h + 1;
            for y in y0..(y0 + cell_h - 2).min(height as usize) {
                for x in x0..(x0 + cell_w - 2).min(width as usize) {
                    let idx = (y * width as usize + x) * 4;
                    rgba[idx..idx + 4].copy_from_slice(&color);
                }
            }
        }
    }
    write_rgba_png(path, width, height, &rgba)?;
    analyze_png_file(path)
}

fn write_rgba_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(path)
        .map(BufWriter::new)
        .map_err(anyhow::Error::from)?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    Ok(())
}

fn write_visible_screenshot_png(data_url: Option<&str>, path: &Path) -> Result<ImageArtifactProof> {
    let data_url = data_url.context("Firefox proof did not include visible screenshot data")?;
    let bytes = decode_png_data_url(data_url)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    analyze_png_file(path)
}

fn decode_png_data_url(data_url: &str) -> Result<Vec<u8>> {
    let payload = data_url
        .strip_prefix("data:image/png;base64,")
        .context("visible screenshot was not a PNG data URL")?;
    base64::engine::general_purpose::STANDARD
        .decode(payload)
        .context("decoding visible screenshot PNG data URL")
}

fn analyze_png_file(path: &Path) -> Result<ImageArtifactProof> {
    let bytes = fs::read(path).with_context(|| format!("reading PNG {}", path.display()))?;
    let decoder = png::Decoder::new(BufReader::new(Cursor::new(bytes.as_slice())));
    let mut reader = decoder.read_info()?;
    let mut buf = vec![
        0;
        reader
            .output_buffer_size()
            .context("PNG output buffer is too large")?
    ];
    let info = reader.next_frame(&mut buf)?;
    let data = &buf[..info.buffer_size()];
    let rgba = png_to_rgba(data, info.color_type)?;
    let (sampled_colors, dominant_rgba, dominant_ratio) = image_color_stats(&rgba);
    let nonblank = rgba.iter().any(|byte| *byte != 0) && sampled_colors > 1;
    let not_error_solid = !is_error_solid(dominant_rgba, dominant_ratio, sampled_colors);
    Ok(ImageArtifactProof {
        path: path.to_path_buf(),
        width: info.width,
        height: info.height,
        byte_len: bytes.len(),
        png_sha256: hex::encode(Sha256::digest(&bytes)),
        sampled_colors,
        dominant_rgba,
        dominant_ratio,
        nonblank,
        not_error_solid,
        passed: info.width > 0
            && info.height > 0
            && bytes.len() > 32
            && nonblank
            && not_error_solid,
    })
}

fn png_to_rgba(data: &[u8], color_type: png::ColorType) -> Result<Vec<u8>> {
    let mut rgba = Vec::new();
    match color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(data),
        png::ColorType::Rgb => {
            for rgb in data.chunks_exact(3) {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for gray in data {
                rgba.extend_from_slice(&[*gray, *gray, *gray, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for gray_alpha in data.chunks_exact(2) {
                rgba.extend_from_slice(&[
                    gray_alpha[0],
                    gray_alpha[0],
                    gray_alpha[0],
                    gray_alpha[1],
                ]);
            }
        }
        png::ColorType::Indexed => bail!("indexed PNG screenshots are not supported"),
    }
    Ok(rgba)
}

fn image_color_stats(rgba: &[u8]) -> (usize, [u8; 4], f64) {
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

fn is_error_solid(color: [u8; 4], ratio: f64, sampled_colors: usize) -> bool {
    if sampled_colors <= 1 && ratio > 0.99 {
        return true;
    }
    if ratio < 0.985 {
        return false;
    }
    let [r, g, b, _] = color;
    let mostly_black = r < 16 && g < 16 && b < 16;
    let mostly_white = r > 240 && g > 240 && b > 240;
    let mostly_red = r > 180 && g < 80 && b < 80;
    mostly_black || mostly_white || mostly_red
}

fn replay_ratatui(
    name: &str,
    _pty: bool,
    expected_snapshot: &boon_runtime::AppSnapshot,
    expected_frame_hash: &str,
) -> Result<ReplayProof> {
    let mut app = app(name)?;
    let mut backend = RatatuiBackend::new(120, 40);
    backend.load(&mut app)?;
    run_core_scenario(name, &mut app, &mut backend)?;
    let _ = ratatui_timing_gate(name, &mut app, &mut backend)?;
    let frame = backend.capture_frame()?;
    let replay_frame_hash = stable_sha(&frame.text);
    let expected_snapshot_hash = snapshot_hash(expected_snapshot)?;
    let replay_snapshot_hash = snapshot_hash(&app.snapshot())?;
    Ok(ReplayProof {
        passed: expected_snapshot_hash == replay_snapshot_hash
            && expected_frame_hash == replay_frame_hash,
        snapshot_hash: expected_snapshot_hash,
        replay_snapshot_hash,
        frame_hash: Some(expected_frame_hash.to_string()),
        replay_frame_hash: Some(replay_frame_hash),
        steps: replay_steps(name),
    })
}

fn replay_wgpu(
    name: &str,
    expected_snapshot: &boon_runtime::AppSnapshot,
    expected_frame_hash: Option<&str>,
) -> Result<ReplayProof> {
    let mut app = app(name)?;
    let mut backend = WgpuBackend::headless_real(1280, 720)?;
    backend.load(&mut app)?;
    run_core_scenario_wgpu(name, &mut app, &mut backend)?;
    let _ = browser_timing_gate(name, &mut app, &mut backend)?;
    let frame = backend.capture_frame()?;
    let expected_snapshot_hash = snapshot_hash(expected_snapshot)?;
    let replay_snapshot_hash = snapshot_hash(&app.snapshot())?;
    Ok(ReplayProof {
        passed: expected_snapshot_hash == replay_snapshot_hash
            && expected_frame_hash == frame.rgba_hash.as_deref(),
        snapshot_hash: expected_snapshot_hash,
        replay_snapshot_hash,
        frame_hash: expected_frame_hash.map(str::to_string),
        replay_frame_hash: frame.rgba_hash,
        steps: replay_steps(name),
    })
}

fn snapshot_hash(snapshot: &boon_runtime::AppSnapshot) -> Result<String> {
    Ok(stable_sha(&serde_json::to_string(snapshot)?))
}

pub fn scenario_for_example(name: &str) -> Scenario {
    let base = Scenario::new(name)
        .mount()
        .expect_visible_output()
        .expect_source_inventory()
        .expect_replay()
        .expect_frame_hash();
    match name {
        "counter" | "counter_hold" => base
            .click("increment button")
            .click("increment button")
            .click("increment button")
            .click("increment button")
            .click("increment button")
            .click("increment button")
            .click("increment button")
            .click("increment button")
            .click("increment button")
            .click("increment button")
            .timing("counter_click_30")
            .expect_state("counter")
            .expect_source_binding("store.sources.increment_button.event.press")
            .expect_timing_budget("counter_click_30"),
        "interval" | "interval_hold" => base
            .advance_clock(3000)
            .timing("interval_clock_30")
            .expect_state("interval_count")
            .expect_source_binding("store.sources.tick.event.frame")
            .expect_timing_budget("interval_clock_30"),
        "todo_mvc" | "todo_mvc_physical" => base
            .focus("new_todo_input")
            .type_text("new_todo_input", "Buy groceries")
            .change("new_todo_input")
            .key_down("new_todo_input", "Enter")
            .type_text("new_todo_input", "   ")
            .key_down("new_todo_input", "Enter")
            .expect_error_rejected("whitespace-only todo is rejected")
            .type_text("todo edit input", "Buy groceries")
            .key_down("todo edit input", "Enter")
            .blur("todo edit input")
            .click("toggle-all checkbox")
            .click("todo item checkbox")
            .click("completed filter")
            .click("active filter")
            .click("all filter")
            .click("todo item remove button")
            .click("clear-completed button")
            .timing("todomvc_typing_100")
            .timing("todomvc_check_one_item_30")
            .timing("todomvc_toggle_all_30")
            .expect_state("store.todos_count")
            .expect_source_binding("store.sources.new_todo_input.text")
            .expect_source_binding("todos[*].sources.checkbox.event.click")
            .expect_timing_budget("todomvc_typing_100")
            .expect_timing_budget("todomvc_check_one_item_30")
            .expect_timing_budget("todomvc_toggle_all_30"),
        "cells" => base
            .click("A1 cell display")
            .type_text("A1 editor", "1")
            .key_down("A1 editor", "Enter")
            .type_text("A2 editor", "2")
            .type_text("A3 editor", "3")
            .type_text("B1 editor", "=add(A1, A2)")
            .type_text("B2 editor", "=sum(A1:A3)")
            .key_down("viewport", "ArrowDown")
            .key_down("viewport", "ArrowRight")
            .type_text("Z100 editor", "edge")
            .expect_error_rejected("invalid and cyclic formulas are visible errors")
            .timing("cells_edit_a1_30")
            .timing("cells_edit_a2_dependents_30")
            .timing("cells_viewport_z100_30")
            .expect_state("cells.A1")
            .expect_source_binding("cells[*].sources.editor.text")
            .expect_timing_budget("cells_edit_a1_30")
            .expect_timing_budget("cells_edit_a2_dependents_30")
            .expect_timing_budget("cells_viewport_z100_30"),
        "pong" | "arkanoid" => base
            .key_down("paddle", "ArrowUp")
            .key_down("paddle", "ArrowDown")
            .advance_frame("tick")
            .timing("game_frame_30")
            .expect_source_binding("store.sources.paddle.event.key_down.key")
            .expect_timing_budget("game_frame_30"),
        _ => base.expect_error_rejected("unknown example has no maintained scenario"),
    }
}

fn replay_steps(name: &str) -> Vec<String> {
    scenario_for_example(name).replay_steps()
}

fn human_like_interactions(name: &str) -> Vec<String> {
    scenario_for_example(name).human_steps()
}

pub fn verify_native_wgpu_headless(artifacts: &Path) -> Result<VerifyReport> {
    let mut results = Vec::new();
    for name in list_examples() {
        let mut app = app(name)?;
        let mut backend = WgpuBackend::headless_real(1280, 720)?;
        let dir = artifacts.join(name).join("native-wgpu-headless");
        fs::create_dir_all(&dir)?;
        match backend.load(&mut app) {
            Ok(info) => {
                run_core_scenario_wgpu(name, &mut app, &mut backend)?;
                let timing = browser_timing_gate(name, &mut app, &mut backend)?;
                let frame = backend.capture_frame()?;
                let frame_png = write_wgpu_frame_png(&backend, &dir, "frame.png")?;
                fs::write(
                    dir.join("timings.json"),
                    serde_json::to_vec_pretty(&timing)?,
                )?;
                fs::write(
                    dir.join("trace.json"),
                    serde_json::to_vec_pretty(&json!({
                        "example": name,
                        "mode": "native-wgpu-headless",
                        "scenario_builder": scenario_for_example(name),
                        "initial_hash": info.hash,
                        "final_rgba_hash": frame.rgba_hash,
                        "frame_png": &frame_png,
                        "metadata": backend.metadata(),
                        "source_inventory": app.source_inventory(),
                        "snapshot": app.snapshot(),
                    }))?,
                )?;
                let replay = replay_wgpu(name, &app.snapshot(), frame.rgba_hash.as_deref())?;
                fs::write(dir.join("replay.json"), serde_json::to_vec_pretty(&replay)?)?;
                let timing_passed = timing
                    .get("passed")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(true);
                results.push(GateResult {
                    backend: Backend::NativeWgpuHeadless,
                    example: (*name).to_string(),
                    passed: timing_passed && replay.passed && frame_png.nonblank,
                    frame_hash: frame.rgba_hash,
                    artifact_dir: dir,
                    message: if timing_passed && replay.passed && frame_png.nonblank {
                        format!(
                        "passed native wgpu adapter/device, offscreen render, framebuffer readback, PNG frame artifact, deterministic scenario checks, and replay gate ({:?})",
                        backend.metadata()
                    )
                    } else if !replay.passed {
                        "native wgpu replay gate failed".to_string()
                    } else if !frame_png.nonblank {
                        "native wgpu frame PNG artifact check failed".to_string()
                    } else {
                        "native wgpu timing budget gate failed".to_string()
                    },
                });
            }
            Err(err) => {
                fs::write(dir.join("failure.txt"), err.to_string())?;
                results.push(GateResult {
                    backend: Backend::NativeWgpuHeadless,
                    example: (*name).to_string(),
                    passed: false,
                    frame_hash: None,
                    artifact_dir: dir,
                    message: err.to_string(),
                });
                break;
            }
        }
    }
    Ok(VerifyReport {
        command: "verify native-wgpu --headless".to_string(),
        results,
    })
}

pub fn verify_all(artifacts: &Path) -> Result<VerifyReport> {
    let mut results = Vec::new();
    results.extend(verify_ratatui(artifacts, false)?.results);
    results.extend(verify_ratatui(artifacts, true)?.results);
    let native = verify_native_wgpu_headless(artifacts)?;
    let failed = native.results.iter().any(|r| !r.passed);
    results.extend(native.results);
    if failed {
        return Ok(VerifyReport {
            command: "verify all".to_string(),
            results,
        });
    }
    let app_window = verify_native_app_window(artifacts)?;
    let failed = app_window.results.iter().any(|r| !r.passed);
    results.extend(app_window.results);
    if failed {
        return Ok(VerifyReport {
            command: "verify all".to_string(),
            results,
        });
    }
    let browser = verify_browser_firefox(artifacts)?;
    let failed = browser.results.iter().any(|r| !r.passed);
    results.extend(browser.results);
    if failed {
        return Ok(VerifyReport {
            command: "verify all".to_string(),
            results,
        });
    }
    Ok(VerifyReport {
        command: "verify all".to_string(),
        results,
    })
}

pub fn verify_browser_firefox(artifacts: &Path) -> Result<VerifyReport> {
    let root_dir = artifacts.join("browser-firefox-extension");
    fs::create_dir_all(&root_dir)?;
    match boon_backend_browser::doctor_firefox_webgpu() {
        Ok(capability) => {
            fs::write(
                root_dir.join("doctor.json"),
                serde_json::to_vec_pretty(&capability)?,
            )?;
            let mut pending = Vec::new();
            for name in list_examples() {
                let mut app = app(name)?;
                let mut backend = WgpuBackend::headless_real(1280, 720)?;
                let dir = artifacts.join(name).join("browser-firefox-extension");
                fs::create_dir_all(&dir)?;
                let initial = backend.load(&mut app)?;
                run_core_scenario_wgpu(name, &mut app, &mut backend)?;
                let timing = browser_timing_gate(name, &mut app, &mut backend)?;
                let frame = backend.capture_frame()?;
                let expected_frame_png =
                    write_wgpu_frame_png(&backend, &dir, "expected-frame.png")?;
                pending.push((
                    (*name).to_string(),
                    initial.hash,
                    frame.rgba_hash.clone(),
                    expected_frame_png,
                    serde_json::to_value(backend.metadata())?,
                    serde_json::to_value(app.source_inventory())?,
                    serde_json::to_value(app.snapshot())?,
                    timing,
                ));
            }

            let scenario_inputs = pending
                .iter()
                .map(
                    |(name, _, frame_hash, _, metadata, source_inventory, snapshot, timing)| {
                        Ok::<_, anyhow::Error>(BrowserScenarioInput {
                            example: name.clone(),
                            snapshot: snapshot.clone(),
                            source_inventory: source_inventory.clone(),
                            frame_hash: frame_hash.clone(),
                            timing: timing.clone(),
                            wgpu_metadata: metadata.clone(),
                            scenario: serde_json::to_value(scenario_for_example(name))?,
                        })
                    },
                )
                .collect::<Result<Vec<_>>>()?;
            let browser_proofs =
                boon_backend_browser::run_firefox_webgpu_scenarios(&scenario_inputs)?;
            fs::write(
                root_dir.join("scenario-proofs.json"),
                serde_json::to_vec_pretty(&browser_proofs_without_screenshot_data(
                    &browser_proofs,
                ))?,
            )?;

            let mut results = Vec::new();
            for (
                name,
                initial_hash,
                frame_hash,
                expected_frame_png,
                metadata,
                source_inventory,
                snapshot,
                timing,
            ) in pending
            {
                let dir = artifacts.join(&name).join("browser-firefox-extension");
                fs::create_dir_all(&dir)?;
                let proof = browser_proofs
                    .iter()
                    .find(|proof| proof.example == name)
                    .cloned();
                let browser_frame_hash = proof
                    .as_ref()
                    .and_then(|proof| proof.frame_hash.clone())
                    .or_else(|| frame_hash.clone());
                let visible_screenshot = proof
                    .as_ref()
                    .map(|proof| {
                        write_visible_screenshot_png(
                            proof.visible_screenshot_png_data_url.as_deref(),
                            &dir.join("visible-screenshot.png"),
                        )
                    })
                    .transpose()?;
                let proof_for_trace = proof.as_ref().map(browser_proof_without_screenshot_data);
                fs::write(
                    dir.join("trace.json"),
                    serde_json::to_vec_pretty(&json!({
                        "example": name,
                        "mode": "browser-firefox-webgpu-extension",
                        "firefox": capability,
                        "initial_hash": initial_hash,
                        "native_reference_rgba_hash": frame_hash,
                        "final_rgba_hash": browser_frame_hash,
                        "scenario_builder": scenario_for_example(&name),
                        "expected_frame_png": &expected_frame_png,
                        "visible_screenshot": &visible_screenshot,
                        "metadata": metadata,
                        "source_inventory": source_inventory,
                        "snapshot": snapshot,
                        "browser_proof": proof_for_trace,
                        "scenario": "firefox-webgpu-webextension-test-api",
                    }))?,
                )?;
                fs::write(
                    dir.join("timings.json"),
                    serde_json::to_vec_pretty(&timing)?,
                )?;
                if let Some(proof) = &proof {
                    let proof_for_disk = browser_proof_without_screenshot_data(proof);
                    fs::write(
                        dir.join("browser-proof.json"),
                        serde_json::to_vec_pretty(&proof_for_disk)?,
                    )?;
                    fs::write(
                        dir.join("replay.json"),
                        serde_json::to_vec_pretty(&json!({
                            "passed": proof.wasm_runner_ok
                                && proof.wasm_frame_hash == proof.frame_hash
                                && proof.wasm_source_count == proof.source_count
                                && proof.errors.is_empty(),
                            "kind": "firefox-wasm-proof-replay",
                            "frame_hash": proof.frame_hash,
                            "native_reference_frame_hash": frame_hash,
                            "wasm_frame_hash": proof.wasm_frame_hash,
                            "source_count": proof.source_count,
                            "wasm_source_count": proof.wasm_source_count,
                            "wasm_snapshot_matches": proof.wasm_snapshot_matches,
                            "wasm_source_inventory_matches": proof.wasm_source_inventory_matches,
                            "steps": replay_steps(&name),
                            "scenario_builder": scenario_for_example(&name),
                        }))?,
                    )?;
                }
                let passed = browser_frame_hash.is_some()
                    && browser_frame_hash.as_deref() != Some("")
                    && frame_hash.is_some()
                    && frame_hash.as_deref() != Some("")
                    && expected_frame_png.nonblank
                    && visible_screenshot
                        .as_ref()
                        .is_some_and(|screenshot| screenshot.passed)
                    && timing
                        .get("passed")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(true)
                    && proof.as_ref().is_some_and(|proof| {
                        proof.navigator_gpu
                            && proof.extension_loaded
                            && proof.native_messaging_connected
                            && proof.test_api_available
                            && proof.test_api_rgba_capture_available
                            && proof.test_api_rgba_hash == proof.frame_hash
                            && proof.test_api_rgba_byte_length == 1280 * 720 * 4
                            && proof.test_api_rgba_distinct_sampled_colors > 1
                            && proof.scenario_action_count
                                == scenario_for_example(&name).steps.len()
                            && proof.scenario_actions_accepted
                            && proof.wasm_loaded
                            && proof.wasm_runner_ok
                            && proof.wasm_snapshot_matches
                            && proof.wasm_source_inventory_matches
                            && proof.adapter_requested
                            && proof.device_requested
                            && proof.gpu_buffer_bytes >= 16
                            && proof.source_count > 0
                            && proof.wasm_source_count == proof.source_count
                            && proof.wasm_snapshot_values > 0
                            && proof.frame_hash.is_some()
                            && proof.wasm_frame_hash == proof.frame_hash
                            && proof.visible_screenshot_source.as_deref()
                                == Some("firefox-tabs-api")
                            && proof.timing_passed
                            && proof.errors.is_empty()
                    });
                results.push(GateResult {
                    backend: Backend::BrowserFirefoxWgpu,
                    example: name,
                    passed,
                    frame_hash: browser_frame_hash,
                    artifact_dir: dir,
                    message: if passed {
                        "passed real Firefox WebGPU/WebExtension/native-messaging/Rust-wasm test API proof plus visible screenshot, PNG frame artifact, deterministic state, source inventory, frame hash, and timing gate".to_string()
                    } else {
                        "Firefox browser scenario or timing gate failed".to_string()
                    },
                });
                if !passed {
                    break;
                }
            }
            Ok(VerifyReport {
                command: "verify browser-wgpu --browser firefox".to_string(),
                results,
            })
        }
        Err(err) => {
            fs::write(root_dir.join("failure.txt"), err.to_string())?;
            Ok(VerifyReport {
                command: "verify browser-wgpu --browser firefox".to_string(),
                results: vec![GateResult {
                    backend: Backend::BrowserFirefoxWgpu,
                    example: "all".to_string(),
                    passed: false,
                    frame_hash: None,
                    artifact_dir: root_dir,
                    message: err.to_string(),
                }],
            })
        }
    }
}

fn browser_proofs_without_screenshot_data(
    proofs: &[boon_backend_browser::BrowserScenarioProof],
) -> Vec<boon_backend_browser::BrowserScenarioProof> {
    proofs
        .iter()
        .map(browser_proof_without_screenshot_data)
        .collect()
}

fn browser_proof_without_screenshot_data(
    proof: &boon_backend_browser::BrowserScenarioProof,
) -> boon_backend_browser::BrowserScenarioProof {
    let mut proof = proof.clone();
    proof.visible_screenshot_png_data_url = None;
    proof
}

fn browser_timing_gate(
    name: &str,
    app: &mut impl BoonApp,
    backend: &mut WgpuBackend,
) -> Result<serde_json::Value> {
    match name {
        "todo_mvc" | "todo_mvc_physical" => {
            let mut cases = Vec::new();
            cases.push(measure_repeated_dispatch_n(
                app,
                backend,
                "todomvc_typing_100",
                8.0,
                16.0,
                None,
                100,
                |i| {
                    state(
                        "store.sources.new_todo_input.text",
                        SourceValue::Text("x".repeat(i + 1)),
                    )
                },
            )?);
            expect(
                app.snapshot()
                    .values
                    .get("store.sources.new_todo_input.text"),
                json!("x".repeat(105)),
                "store.sources.new_todo_input.text after timing",
            )?;
            ensure_todo_count_wgpu(app, backend, 100)?;
            cases.push(measure_repeated_dispatch(
                app,
                backend,
                "todomvc_check_one_item_30",
                5.0,
                10.0,
                None,
                |i| {
                    let owner_id = if i == 0 { 1 } else { i + 3 }.to_string();
                    dynamic_event(
                        "todos[*].sources.checkbox.event.click",
                        &owner_id,
                        0,
                        SourceValue::EmptyRecord,
                    )
                },
            )?);
            cases.push(measure_repeated_dispatch(
                app,
                backend,
                "todomvc_toggle_all_30",
                10.0,
                16.0,
                Some(25.0),
                |_| {
                    event(
                        "store.sources.toggle_all_checkbox.event.click",
                        SourceValue::EmptyRecord,
                    )
                },
            )?);
            Ok(timing_cases(cases))
        }
        "cells" => {
            backend.dispatch_frame_ready(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A1",
                    0,
                    SourceValue::Text("1".to_string()),
                ),
            )?;
            backend.dispatch_frame_ready(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A2",
                    0,
                    SourceValue::Text("2".to_string()),
                ),
            )?;
            backend.dispatch_frame_ready(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A3",
                    0,
                    SourceValue::Text("3".to_string()),
                ),
            )?;
            backend.dispatch_frame_ready(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "B1",
                    0,
                    SourceValue::Text("=add(A1, A2)".to_string()),
                ),
            )?;
            backend.dispatch_frame_ready(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "B2",
                    0,
                    SourceValue::Text("=sum(A1:A3)".to_string()),
                ),
            )?;
            let cases = vec![
                measure_repeated_dispatch(
                    app,
                    backend,
                    "cells_edit_a1_30",
                    8.0,
                    16.0,
                    None,
                    |i| {
                        dynamic_state(
                            "cells[*].sources.editor.text",
                            "A1",
                            0,
                            SourceValue::Text(i.to_string()),
                        )
                    },
                )?,
                measure_repeated_dispatch(
                    app,
                    backend,
                    "cells_edit_a2_dependents_30",
                    10.0,
                    16.0,
                    None,
                    |i| {
                        dynamic_state(
                            "cells[*].sources.editor.text",
                            "A2",
                            0,
                            SourceValue::Text((i + 2).to_string()),
                        )
                    },
                )?,
                measure_repeated_dispatch(
                    app,
                    backend,
                    "cells_viewport_z100_30",
                    10.0,
                    20.0,
                    None,
                    |i| {
                        dynamic_state(
                            "cells[*].sources.editor.text",
                            "Z100",
                            0,
                            SourceValue::Text(format!("edge-{i}")),
                        )
                    },
                )?,
            ];
            Ok(timing_cases(cases))
        }
        "counter" | "counter_hold" => Ok(timing_cases(vec![measure_repeated_dispatch(
            app,
            backend,
            "counter_click_30",
            5.0,
            10.0,
            None,
            |_| {
                event(
                    "store.sources.increment_button.event.press",
                    SourceValue::EmptyRecord,
                )
            },
        )?])),
        "interval" | "interval_hold" => Ok(timing_cases(vec![measure_repeated_wgpu_operation(
            app,
            backend,
            "interval_clock_30",
            5.0,
            10.0,
            None,
            30,
            |app, backend, _| {
                let result = app.advance_fake_time(Duration::from_millis(16));
                backend.apply_patches(&result.patches)?;
                backend.render_frame_ready()?;
                Ok(())
            },
        )?])),
        "pong" | "arkanoid" => Ok(timing_cases(vec![measure_repeated_dispatch(
            app,
            backend,
            "game_frame_30",
            5.0,
            10.0,
            None,
            |_| event("store.sources.tick.event.frame", SourceValue::EmptyRecord),
        )?])),
        _ => Ok(json!({
            "passed": false,
            "error": format!("no timing gate for {name}"),
        })),
    }
}

fn ratatui_timing_gate(
    name: &str,
    app: &mut impl BoonApp,
    backend: &mut RatatuiBackend,
) -> Result<serde_json::Value> {
    match name {
        "todo_mvc" | "todo_mvc_physical" => {
            let mut cases = Vec::new();
            cases.push(measure_repeated_dispatch_ratatui_n(
                app,
                backend,
                "todomvc_typing_100",
                8.0,
                16.0,
                None,
                100,
                |i| {
                    state(
                        "store.sources.new_todo_input.text",
                        SourceValue::Text("x".repeat(i + 1)),
                    )
                },
            )?);
            expect(
                app.snapshot()
                    .values
                    .get("store.sources.new_todo_input.text"),
                json!("x".repeat(105)),
                "store.sources.new_todo_input.text after timing",
            )?;
            ensure_todo_count_ratatui(app, backend, 100)?;
            cases.push(measure_repeated_dispatch_ratatui(
                app,
                backend,
                "todomvc_check_one_item_30",
                5.0,
                10.0,
                None,
                |i| {
                    let owner_id = if i == 0 { 1 } else { i + 3 }.to_string();
                    dynamic_event(
                        "todos[*].sources.checkbox.event.click",
                        &owner_id,
                        0,
                        SourceValue::EmptyRecord,
                    )
                },
            )?);
            cases.push(measure_repeated_dispatch_ratatui(
                app,
                backend,
                "todomvc_toggle_all_30",
                10.0,
                16.0,
                Some(25.0),
                |_| {
                    event(
                        "store.sources.toggle_all_checkbox.event.click",
                        SourceValue::EmptyRecord,
                    )
                },
            )?);
            Ok(timing_cases(cases))
        }
        "cells" => {
            backend.dispatch(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A1",
                    0,
                    SourceValue::Text("1".to_string()),
                ),
            )?;
            backend.dispatch(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A2",
                    0,
                    SourceValue::Text("2".to_string()),
                ),
            )?;
            backend.dispatch(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A3",
                    0,
                    SourceValue::Text("3".to_string()),
                ),
            )?;
            backend.dispatch(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "B1",
                    0,
                    SourceValue::Text("=add(A1, A2)".to_string()),
                ),
            )?;
            backend.dispatch(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "B2",
                    0,
                    SourceValue::Text("=sum(A1:A3)".to_string()),
                ),
            )?;
            let cases = vec![
                measure_repeated_dispatch_ratatui(
                    app,
                    backend,
                    "cells_edit_a1_30",
                    8.0,
                    16.0,
                    None,
                    |i| {
                        dynamic_state(
                            "cells[*].sources.editor.text",
                            "A1",
                            0,
                            SourceValue::Text(i.to_string()),
                        )
                    },
                )?,
                measure_repeated_dispatch_ratatui(
                    app,
                    backend,
                    "cells_edit_a2_dependents_30",
                    10.0,
                    16.0,
                    None,
                    |i| {
                        dynamic_state(
                            "cells[*].sources.editor.text",
                            "A2",
                            0,
                            SourceValue::Text((i + 2).to_string()),
                        )
                    },
                )?,
                measure_repeated_dispatch_ratatui(
                    app,
                    backend,
                    "cells_viewport_z100_30",
                    10.0,
                    20.0,
                    None,
                    |i| {
                        dynamic_state(
                            "cells[*].sources.editor.text",
                            "Z100",
                            0,
                            SourceValue::Text(format!("edge-{i}")),
                        )
                    },
                )?,
            ];
            Ok(timing_cases(cases))
        }
        "counter" | "counter_hold" => Ok(timing_cases(vec![measure_repeated_dispatch_ratatui(
            app,
            backend,
            "counter_click_30",
            5.0,
            10.0,
            None,
            |_| {
                event(
                    "store.sources.increment_button.event.press",
                    SourceValue::EmptyRecord,
                )
            },
        )?])),
        "interval" | "interval_hold" => Ok(timing_cases(vec![measure_repeated_ratatui_operation(
            app,
            backend,
            "interval_clock_30",
            5.0,
            10.0,
            None,
            30,
            |app, backend, _| {
                let result = app.advance_fake_time(Duration::from_millis(16));
                backend.apply_patches(&result.patches)?;
                backend.render_frame()?;
                Ok(())
            },
        )?])),
        "pong" | "arkanoid" => Ok(timing_cases(vec![measure_repeated_dispatch_ratatui(
            app,
            backend,
            "game_frame_30",
            5.0,
            10.0,
            None,
            |_| event("store.sources.tick.event.frame", SourceValue::EmptyRecord),
        )?])),
        _ => Ok(json!({
            "passed": false,
            "error": format!("no timing gate for {name}"),
        })),
    }
}

fn measure_repeated_dispatch(
    app: &mut impl BoonApp,
    backend: &mut WgpuBackend,
    scenario: &str,
    p95_budget_ms: f64,
    p99_budget_ms: f64,
    max_budget_ms: Option<f64>,
    make_batch: impl FnMut(usize) -> SourceBatch,
) -> Result<serde_json::Value> {
    measure_repeated_dispatch_n(
        app,
        backend,
        scenario,
        p95_budget_ms,
        p99_budget_ms,
        max_budget_ms,
        30,
        make_batch,
    )
}

#[allow(clippy::too_many_arguments)]
fn measure_repeated_dispatch_n(
    app: &mut impl BoonApp,
    backend: &mut WgpuBackend,
    scenario: &str,
    p95_budget_ms: f64,
    p99_budget_ms: f64,
    max_budget_ms: Option<f64>,
    measured_iterations: usize,
    mut make_batch: impl FnMut(usize) -> SourceBatch,
) -> Result<serde_json::Value> {
    for i in 0..5 {
        backend.dispatch_frame_ready(app, make_batch(i))?;
    }
    let mut samples = Vec::new();
    for i in 0..measured_iterations {
        let start = Instant::now();
        backend.dispatch_frame_ready(app, make_batch(i + 5))?;
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let p95 = percentile(&sorted, 0.95);
    let p99 = percentile(&sorted, 0.99);
    let max = sorted.last().copied().unwrap_or(0.0);
    let max_pass = max_budget_ms.is_none_or(|budget| max <= budget);
    let passed = p95 <= p95_budget_ms && p99 <= p99_budget_ms && max_pass;
    Ok(json!({
        "scenario": scenario,
        "seed": 1,
        "warmup_iterations": 5,
        "measured_iterations": measured_iterations,
        "samples_ms": samples,
        "p95_ms": p95,
        "p99_ms": p99,
        "max_ms": max,
        "budgets_ms": {
            "p95": p95_budget_ms,
            "p99": p99_budget_ms,
            "max": max_budget_ms,
        },
        "passed": passed,
    }))
}

#[allow(clippy::too_many_arguments)]
fn measure_repeated_wgpu_operation<A: BoonApp>(
    app: &mut A,
    backend: &mut WgpuBackend,
    scenario: &str,
    p95_budget_ms: f64,
    p99_budget_ms: f64,
    max_budget_ms: Option<f64>,
    measured_iterations: usize,
    mut operation: impl FnMut(&mut A, &mut WgpuBackend, usize) -> Result<()>,
) -> Result<serde_json::Value> {
    for i in 0..5 {
        operation(app, backend, i)?;
    }
    let mut samples = Vec::new();
    for i in 0..measured_iterations {
        let start = Instant::now();
        operation(app, backend, i + 5)?;
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    timing_sample(
        scenario,
        p95_budget_ms,
        p99_budget_ms,
        max_budget_ms,
        samples,
    )
}

fn measure_repeated_dispatch_ratatui(
    app: &mut impl BoonApp,
    backend: &mut RatatuiBackend,
    scenario: &str,
    p95_budget_ms: f64,
    p99_budget_ms: f64,
    max_budget_ms: Option<f64>,
    make_batch: impl FnMut(usize) -> SourceBatch,
) -> Result<serde_json::Value> {
    measure_repeated_dispatch_ratatui_n(
        app,
        backend,
        scenario,
        p95_budget_ms,
        p99_budget_ms,
        max_budget_ms,
        30,
        make_batch,
    )
}

#[allow(clippy::too_many_arguments)]
fn measure_repeated_dispatch_ratatui_n(
    app: &mut impl BoonApp,
    backend: &mut RatatuiBackend,
    scenario: &str,
    p95_budget_ms: f64,
    p99_budget_ms: f64,
    max_budget_ms: Option<f64>,
    measured_iterations: usize,
    mut make_batch: impl FnMut(usize) -> SourceBatch,
) -> Result<serde_json::Value> {
    for i in 0..5 {
        backend.dispatch(app, make_batch(i))?;
    }
    let mut samples = Vec::new();
    for i in 0..measured_iterations {
        let start = Instant::now();
        backend.dispatch(app, make_batch(i + 5))?;
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let p95 = percentile(&sorted, 0.95);
    let p99 = percentile(&sorted, 0.99);
    let max = sorted.last().copied().unwrap_or(0.0);
    let max_pass = max_budget_ms.is_none_or(|budget| max <= budget);
    let passed = p95 <= p95_budget_ms && p99 <= p99_budget_ms && max_pass;
    Ok(json!({
        "scenario": scenario,
        "seed": 1,
        "warmup_iterations": 5,
        "measured_iterations": measured_iterations,
        "samples_ms": samples,
        "p95_ms": p95,
        "p99_ms": p99,
        "max_ms": max,
        "budgets_ms": {
            "p95": p95_budget_ms,
            "p99": p99_budget_ms,
            "max": max_budget_ms,
        },
        "passed": passed,
    }))
}

#[allow(clippy::too_many_arguments)]
fn measure_repeated_ratatui_operation<A: BoonApp>(
    app: &mut A,
    backend: &mut RatatuiBackend,
    scenario: &str,
    p95_budget_ms: f64,
    p99_budget_ms: f64,
    max_budget_ms: Option<f64>,
    measured_iterations: usize,
    mut operation: impl FnMut(&mut A, &mut RatatuiBackend, usize) -> Result<()>,
) -> Result<serde_json::Value> {
    for i in 0..5 {
        operation(app, backend, i)?;
    }
    let mut samples = Vec::new();
    for i in 0..measured_iterations {
        let start = Instant::now();
        operation(app, backend, i + 5)?;
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    timing_sample(
        scenario,
        p95_budget_ms,
        p99_budget_ms,
        max_budget_ms,
        samples,
    )
}

fn timing_sample(
    scenario: &str,
    p95_budget_ms: f64,
    p99_budget_ms: f64,
    max_budget_ms: Option<f64>,
    samples: Vec<f64>,
) -> Result<serde_json::Value> {
    let measured_iterations = samples.len();
    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let p95 = percentile(&sorted, 0.95);
    let p99 = percentile(&sorted, 0.99);
    let max = sorted.last().copied().unwrap_or(0.0);
    let max_pass = max_budget_ms.is_none_or(|budget| max <= budget);
    let passed =
        measured_iterations > 0 && p95 <= p95_budget_ms && p99 <= p99_budget_ms && max_pass;
    Ok(json!({
        "scenario": scenario,
        "seed": 1,
        "warmup_iterations": 5,
        "measured_iterations": measured_iterations,
        "samples_ms": samples,
        "p95_ms": p95,
        "p99_ms": p99,
        "max_ms": max,
        "budgets_ms": {
            "p95": p95_budget_ms,
            "p99": p99_budget_ms,
            "max": max_budget_ms,
        },
        "passed": passed,
    }))
}

fn timing_cases(cases: Vec<serde_json::Value>) -> serde_json::Value {
    let passed = cases
        .iter()
        .all(|case| case.get("passed").and_then(|value| value.as_bool()) == Some(true));
    json!({
        "passed": passed,
        "seed": 1,
        "cases": cases,
    })
}

fn ensure_todo_count_wgpu(
    app: &mut impl BoonApp,
    backend: &mut WgpuBackend,
    target: i64,
) -> Result<()> {
    loop {
        let current = app
            .snapshot()
            .values
            .get("store.todos_count")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if current >= target {
            return Ok(());
        }
        let title = format!("Todo {next:03}", next = current + 1);
        backend.dispatch_frame_ready(
            app,
            state(
                "store.sources.new_todo_input.text",
                SourceValue::Text(title),
            ),
        )?;
        backend.dispatch_frame_ready(
            app,
            event(
                "store.sources.new_todo_input.event.key_down.key",
                SourceValue::Tag("Enter".to_string()),
            ),
        )?;
    }
}

fn ensure_todo_count_ratatui(
    app: &mut impl BoonApp,
    backend: &mut RatatuiBackend,
    target: i64,
) -> Result<()> {
    loop {
        let current = app
            .snapshot()
            .values
            .get("store.todos_count")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if current >= target {
            return Ok(());
        }
        let title = format!("Todo {next:03}", next = current + 1);
        backend.dispatch(
            app,
            state(
                "store.sources.new_todo_input.text",
                SourceValue::Text(title),
            ),
        )?;
        backend.dispatch(
            app,
            event(
                "store.sources.new_todo_input.event.key_down.key",
                SourceValue::Tag("Enter".to_string()),
            ),
        )?;
    }
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * quantile).ceil() as usize;
    sorted[index.min(sorted.len() - 1)]
}

pub fn verify_native_app_window(artifacts: &Path) -> Result<VerifyReport> {
    let root_dir = artifacts.join("native-app-window");
    fs::create_dir_all(&root_dir)?;
    let smoke = match app_window_smoke_test_with_title(
        "Boon native app_window verification",
        Duration::ZERO,
    ) {
        Ok(smoke) => smoke,
        Err(err) => {
            fs::write(root_dir.join("failure.txt"), err.to_string())?;
            return Ok(VerifyReport {
                command: "verify native-wgpu --app-window".to_string(),
                results: vec![GateResult {
                    backend: Backend::NativeAppWindow,
                    example: "all".to_string(),
                    passed: false,
                    frame_hash: None,
                    artifact_dir: root_dir,
                    message: err.to_string(),
                }],
            });
        }
    };
    let mut results = Vec::new();
    for name in list_examples() {
        let dir = artifacts.join(name).join("native-app-window");
        fs::create_dir_all(&dir)?;
        match run_native_app_window_example_into(name, &dir, &smoke, Duration::ZERO) {
            Ok(result) => {
                let passed = result.passed;
                results.push(result);
                if !passed {
                    break;
                }
            }
            Err(err) => {
                fs::write(dir.join("failure.txt"), err.to_string())?;
                results.push(GateResult {
                    backend: Backend::NativeAppWindow,
                    example: (*name).to_string(),
                    passed: false,
                    frame_hash: None,
                    artifact_dir: dir,
                    message: err.to_string(),
                });
                break;
            }
        }
    }
    Ok(VerifyReport {
        command: "verify native-wgpu --app-window".to_string(),
        results,
    })
}

pub fn run_native_app_window_example(
    example: &str,
    artifacts: &Path,
    hold: Duration,
) -> Result<GateResult> {
    let dir = artifacts.join(example).join("native-app-window-run");
    fs::create_dir_all(&dir)?;
    let smoke = if hold.is_zero() {
        app_window_smoke_test_with_title(format!("Boon {example} native app_window"), hold)?
    } else {
        let proof = run_native_manual_input_session(example, &dir, hold)?;
        fs::write(
            dir.join("manual-input.json"),
            serde_json::to_vec_pretty(&proof)?,
        )?;
        proof.app_window
    };
    run_native_app_window_example_into(example, &dir, &smoke, hold)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct NativeManualInputProof {
    app_window: boon_backend_app_window::AppWindowSmoke,
    example: String,
    hold_ms: u128,
    samples_seen: usize,
    dispatches: Vec<serde_json::Value>,
    final_snapshot: boon_runtime::AppSnapshot,
    final_frame_hash: Option<String>,
    controls: Vec<String>,
    errors: Vec<String>,
}

#[derive(Clone, Debug)]
enum NativeFocus {
    NewTodo,
    TodoEdit { owner_id: String },
    Cell { owner_id: String },
}

struct NativeManualState {
    example: String,
    app: boon_examples::ExampleApp,
    backend: WgpuBackend,
    focused: Option<NativeFocus>,
    text_buffer: String,
    samples_seen: usize,
    dispatches: Vec<serde_json::Value>,
    errors: Vec<String>,
}

impl NativeManualState {
    fn new(example: &str) -> Result<Self> {
        let mut app = app(example)?;
        let mut backend = WgpuBackend::headless_real(1280, 720)?;
        backend.load(&mut app)?;
        Ok(Self {
            example: example.to_string(),
            app,
            backend,
            focused: None,
            text_buffer: String::new(),
            samples_seen: 0,
            dispatches: Vec::new(),
            errors: Vec::new(),
        })
    }

    fn handle_sample(&mut self, sample: AppWindowInputSample) -> Result<()> {
        self.samples_seen += 1;
        if sample.left_clicked
            && let (Some(x), Some(y)) = (sample.mouse_x, sample.mouse_y)
        {
            self.handle_click(x, y)?;
        }
        for key in &sample.newly_pressed_keys {
            self.handle_key(key, &sample.pressed_keys)?;
        }
        Ok(())
    }

    fn handle_click(&mut self, x: f64, y: f64) -> Result<()> {
        match self.example.as_str() {
            "counter" | "counter_hold" => self.dispatch_labeled(
                "native mouse click increment button",
                event(
                    "store.sources.increment_button.event.press",
                    SourceValue::EmptyRecord,
                ),
            ),
            "interval" | "interval_hold" => {
                let result = self.app.advance_fake_time(Duration::from_secs(1));
                self.backend.apply_patches(&result.patches)?;
                self.backend.render_frame()?;
                self.dispatches.push(json!({
                    "action": "native mouse click advance clock",
                    "batch": "advance_fake_time 1000ms",
                }));
                Ok(())
            }
            "todo_mvc" | "todo_mvc_physical" => self.handle_todo_click(x, y),
            "cells" => self.handle_cells_click(x, y),
            "pong" | "arkanoid" => self.dispatch_labeled(
                "native mouse click frame tick",
                event("store.sources.tick.event.frame", SourceValue::EmptyRecord),
            ),
            _ => Ok(()),
        }
    }

    fn handle_todo_click(&mut self, x: f64, y: f64) -> Result<()> {
        if y < 96.0 {
            self.focused = Some(NativeFocus::NewTodo);
            self.text_buffer = self
                .app
                .snapshot()
                .values
                .get("store.sources.new_todo_input.text")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            if self.has_source("store.sources.new_todo_input.event.focus") {
                self.dispatch_labeled(
                    "native mouse focus new todo input",
                    event(
                        "store.sources.new_todo_input.event.focus",
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            return Ok(());
        }
        if (96.0..136.0).contains(&y)
            && self.has_source("store.sources.toggle_all_checkbox.event.click")
        {
            return self.dispatch_labeled(
                "native mouse click toggle all",
                event(
                    "store.sources.toggle_all_checkbox.event.click",
                    SourceValue::EmptyRecord,
                ),
            );
        }
        if y >= 560.0 {
            if x < 180.0 && self.has_source("store.sources.clear_completed_button.event.press") {
                return self.dispatch_labeled(
                    "native mouse click clear completed",
                    event(
                        "store.sources.clear_completed_button.event.press",
                        SourceValue::EmptyRecord,
                    ),
                );
            }
            let filter = if x < 320.0 {
                "all"
            } else if x < 480.0 {
                "active"
            } else {
                "completed"
            };
            let path = format!("store.sources.filter_{filter}.event.press");
            if self.has_source(&path) {
                return self.dispatch_labeled(
                    &format!("native mouse click {filter} filter"),
                    event(&path, SourceValue::EmptyRecord),
                );
            }
            return Ok(());
        }
        let row = ((y - 152.0) / 40.0).floor() as i64 + 1;
        if row <= 0 {
            return Ok(());
        }
        let owner_id = row.to_string();
        if x < 120.0 && self.has_source("todos[*].sources.checkbox.event.click") {
            return self.dispatch_labeled(
                "native mouse click todo checkbox",
                dynamic_event(
                    "todos[*].sources.checkbox.event.click",
                    &owner_id,
                    0,
                    SourceValue::EmptyRecord,
                ),
            );
        }
        if x > 720.0 && self.has_source("todos[*].sources.remove_button.event.press") {
            return self.dispatch_labeled(
                "native mouse click todo remove button",
                dynamic_event(
                    "todos[*].sources.remove_button.event.press",
                    &owner_id,
                    0,
                    SourceValue::EmptyRecord,
                ),
            );
        }
        self.focused = Some(NativeFocus::TodoEdit {
            owner_id: owner_id.clone(),
        });
        self.text_buffer = self
            .app
            .snapshot()
            .values
            .get(&format!("store.todos[{owner_id}].title"))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        Ok(())
    }

    fn handle_cells_click(&mut self, x: f64, y: f64) -> Result<()> {
        let col = ((x - 80.0) / 96.0).floor() as i64 + 1;
        let row = ((y - 88.0) / 36.0).floor() as i64 + 1;
        if !(1..=26).contains(&col) || !(1..=100).contains(&row) {
            return Ok(());
        }
        let owner_id = format!("{}{}", column_label(col as usize), row);
        self.focused = Some(NativeFocus::Cell {
            owner_id: owner_id.clone(),
        });
        self.text_buffer.clear();
        self.dispatch_labeled(
            "native mouse double-click cell display",
            dynamic_event(
                "cells[*].sources.display.event.double_click",
                &owner_id,
                0,
                SourceValue::EmptyRecord,
            ),
        )
    }

    fn handle_key(&mut self, key: &str, pressed_keys: &[String]) -> Result<()> {
        match key {
            "Return" | "KeypadEnter" => self.dispatch_enter(),
            "Backspace" | "Delete" => {
                self.text_buffer.pop();
                self.dispatch_focused_text()
            }
            "UpArrow" | "DownArrow" | "LeftArrow" | "RightArrow" => {
                let tag = match key {
                    "UpArrow" => "ArrowUp",
                    "DownArrow" => "ArrowDown",
                    "LeftArrow" => "ArrowLeft",
                    "RightArrow" => "ArrowRight",
                    _ => unreachable!(),
                };
                if self.example == "cells" && self.focused.is_none() {
                    return self.dispatch_labeled(
                        "native keyboard grid viewport",
                        event(
                            "store.sources.viewport.event.key_down.key",
                            SourceValue::Tag(tag.to_string()),
                        ),
                    );
                }
                if matches!(self.example.as_str(), "pong" | "arkanoid") {
                    return self.dispatch_labeled(
                        "native keyboard game control",
                        event(
                            "store.sources.paddle.event.key_down.key",
                            SourceValue::Tag(tag.to_string()),
                        ),
                    );
                }
                Ok(())
            }
            _ => {
                if let Some(ch) = key_to_char(key, pressed_keys) {
                    self.text_buffer.push(ch);
                    self.dispatch_focused_text()?;
                }
                Ok(())
            }
        }
    }

    fn dispatch_enter(&mut self) -> Result<()> {
        match self.focused.clone() {
            Some(NativeFocus::NewTodo) => self.dispatch_labeled(
                "native keyboard Enter in new todo input",
                event(
                    "store.sources.new_todo_input.event.key_down.key",
                    SourceValue::Tag("Enter".to_string()),
                ),
            ),
            Some(NativeFocus::TodoEdit { owner_id }) => self.dispatch_labeled(
                "native keyboard Enter in todo edit input",
                dynamic_event(
                    "todos[*].sources.edit_input.event.key_down.key",
                    &owner_id,
                    0,
                    SourceValue::Tag("Enter".to_string()),
                ),
            ),
            Some(NativeFocus::Cell { owner_id }) => self.dispatch_labeled(
                "native keyboard Enter in cell editor",
                dynamic_event(
                    "cells[*].sources.editor.event.key_down.key",
                    &owner_id,
                    0,
                    SourceValue::Tag("Enter".to_string()),
                ),
            ),
            None => Ok(()),
        }
    }

    fn dispatch_focused_text(&mut self) -> Result<()> {
        match self.focused.clone() {
            Some(NativeFocus::NewTodo) => {
                self.dispatch_labeled(
                    "native keyboard text into new todo input",
                    state(
                        "store.sources.new_todo_input.text",
                        SourceValue::Text(self.text_buffer.clone()),
                    ),
                )?;
                if self.has_source("store.sources.new_todo_input.event.change") {
                    self.dispatch_labeled(
                        "native keyboard change in new todo input",
                        event(
                            "store.sources.new_todo_input.event.change",
                            SourceValue::EmptyRecord,
                        ),
                    )?;
                }
                Ok(())
            }
            Some(NativeFocus::TodoEdit { owner_id }) => {
                self.dispatch_labeled(
                    "native keyboard text into todo edit input",
                    dynamic_state(
                        "todos[*].sources.edit_input.text",
                        &owner_id,
                        0,
                        SourceValue::Text(self.text_buffer.clone()),
                    ),
                )?;
                self.dispatch_labeled(
                    "native keyboard change in todo edit input",
                    dynamic_event(
                        "todos[*].sources.edit_input.event.change",
                        &owner_id,
                        0,
                        SourceValue::EmptyRecord,
                    ),
                )?;
                Ok(())
            }
            Some(NativeFocus::Cell { owner_id }) => self.dispatch_labeled(
                "native keyboard text into cell editor",
                dynamic_state(
                    "cells[*].sources.editor.text",
                    &owner_id,
                    0,
                    SourceValue::Text(self.text_buffer.clone()),
                ),
            ),
            None => Ok(()),
        }
    }

    fn dispatch_labeled(&mut self, action: &str, batch: SourceBatch) -> Result<()> {
        let batch_value = serde_json::to_value(&batch)?;
        match self.backend.dispatch_frame_ready(&mut self.app, batch) {
            Ok(info) => {
                self.dispatches.push(json!({
                    "action": action,
                    "batch": batch_value,
                    "frame_hash": info.hash,
                }));
                Ok(())
            }
            Err(err) => {
                self.errors.push(format!("{action}: {err}"));
                Err(err)
            }
        }
    }

    fn has_source(&self, path: &str) -> bool {
        self.app.source_inventory().get(path).is_some()
    }

    fn proof(
        mut self,
        app_window: boon_backend_app_window::AppWindowSmoke,
        hold: Duration,
    ) -> Result<NativeManualInputProof> {
        let frame = self.backend.capture_frame()?;
        Ok(NativeManualInputProof {
            app_window,
            example: self.example.clone(),
            hold_ms: hold.as_millis(),
            samples_seen: self.samples_seen,
            dispatches: self.dispatches,
            final_snapshot: self.app.snapshot(),
            final_frame_hash: frame.rgba_hash,
            controls: native_manual_controls(&self.example),
            errors: self.errors,
        })
    }

    fn current_frame_text(&self) -> String {
        format!(
            "Boon native playground\nexample: {}\n\n{}",
            self.example,
            self.backend.frame_text()
        )
    }
}

fn run_native_manual_input_session(
    example: &str,
    dir: &Path,
    hold: Duration,
) -> Result<NativeManualInputProof> {
    let state = NativeManualState::new(example)?;
    let (app_window, state) = run_text_input_session(
        format!("Boon {example} native app_window"),
        hold,
        Duration::from_millis(16),
        state,
        |state, sample| state.handle_sample(sample),
        |state| Ok(state.current_frame_text()),
    )?;
    let proof = state.proof(app_window, hold)?;
    fs::write(
        dir.join("manual-controls.txt"),
        native_manual_controls(example).join("\n"),
    )?;
    Ok(proof)
}

fn native_manual_controls(example: &str) -> Vec<String> {
    match example {
        "counter" | "counter_hold" => {
            vec!["click anywhere in the native window to increment".to_string()]
        }
        "interval" | "interval_hold" => vec![
            "click anywhere in the native window to advance the fake clock by one second"
                .to_string(),
        ],
        "todo_mvc" | "todo_mvc_physical" => vec![
            "click the top input band, type text, press Enter to add a todo".to_string(),
            "click the toggle-all band below the input".to_string(),
            "click a todo row left side to toggle it".to_string(),
            "click a todo row middle to focus its edit input, type, then press Enter".to_string(),
            "click a todo row far right to remove it".to_string(),
            "click footer bands for clear/all/active/completed where available".to_string(),
        ],
        "cells" => vec![
            "click a visible grid cell to focus it".to_string(),
            "type cell text or formulas and press Enter".to_string(),
            "use arrow keys without a focused cell to move the viewport selection".to_string(),
        ],
        "pong" | "arkanoid" => vec![
            "press arrow keys for paddle controls".to_string(),
            "click the window to advance a deterministic frame".to_string(),
        ],
        _ => Vec::new(),
    }
}

pub fn run_native_playground(initial_example: &str, hold: Duration) -> Result<()> {
    let state = NativePlaygroundState::new(initial_example)?;
    let _ = run_text_input_session(
        "Boon native playground",
        hold,
        Duration::from_millis(16),
        state,
        |state, sample| state.handle_sample(sample),
        |state| Ok(state.current_frame_text()),
    )?;
    Ok(())
}

struct NativePlaygroundState {
    examples: Vec<&'static str>,
    current_index: usize,
    inner: NativeManualState,
    switches: usize,
}

impl NativePlaygroundState {
    fn new(initial_example: &str) -> Result<Self> {
        let examples = list_examples().to_vec();
        let current_index = examples
            .iter()
            .position(|example| *example == initial_example)
            .with_context(|| format!("unknown native playground example `{initial_example}`"))?;
        Ok(Self {
            examples,
            current_index,
            inner: NativeManualState::new(initial_example)?,
            switches: 0,
        })
    }

    fn handle_sample(&mut self, sample: AppWindowInputSample) -> Result<()> {
        for key in &sample.newly_pressed_keys {
            if self.handle_switch_key(key)? {
                return Ok(());
            }
        }
        self.inner.handle_sample(sample)
    }

    fn handle_switch_key(&mut self, key: &str) -> Result<bool> {
        match key {
            "Tab" | "PageDown" => {
                self.switch_to((self.current_index + 1) % self.examples.len())?;
                Ok(true)
            }
            "PageUp" => {
                let next = if self.current_index == 0 {
                    self.examples.len() - 1
                } else {
                    self.current_index - 1
                };
                self.switch_to(next)?;
                Ok(true)
            }
            "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" => {
                let index = key
                    .strip_prefix('F')
                    .and_then(|value| value.parse::<usize>().ok())
                    .and_then(|value| value.checked_sub(1))
                    .unwrap_or(self.current_index);
                if index < self.examples.len() {
                    self.switch_to(index)?;
                    return Ok(true);
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    fn switch_to(&mut self, index: usize) -> Result<()> {
        if index == self.current_index {
            return Ok(());
        }
        self.current_index = index;
        self.inner = NativeManualState::new(self.examples[index])?;
        self.switches += 1;
        Ok(())
    }

    fn current_frame_text(&self) -> String {
        let example_list = self
            .examples
            .iter()
            .enumerate()
            .map(|(index, example)| {
                let marker = if index == self.current_index {
                    ">"
                } else {
                    " "
                };
                format!("{marker} F{} {example}", index + 1)
            })
            .collect::<Vec<_>>()
            .join("  ");
        let controls = native_manual_controls(&self.inner.example).join(" | ");
        format!(
            "Boon native playground\n{} / {}  switches: {}\n{}\n\n{}\n\n{}",
            self.current_index + 1,
            self.examples.len(),
            self.switches,
            example_list,
            self.inner.backend.frame_text(),
            controls
        )
    }
}

fn key_to_char(key: &str, pressed_keys: &[String]) -> Option<char> {
    let shifted = pressed_keys
        .iter()
        .any(|key| matches!(key.as_str(), "Shift" | "RightShift"));
    let ch = match key {
        "A" => 'a',
        "B" => 'b',
        "C" => 'c',
        "D" => 'd',
        "E" => 'e',
        "F" => 'f',
        "G" => 'g',
        "H" => 'h',
        "I" => 'i',
        "J" => 'j',
        "K" => 'k',
        "L" => 'l',
        "M" => 'm',
        "N" => 'n',
        "O" => 'o',
        "P" => 'p',
        "Q" => 'q',
        "R" => 'r',
        "S" => 's',
        "T" => 't',
        "U" => 'u',
        "V" => 'v',
        "W" => 'w',
        "X" => 'x',
        "Y" => 'y',
        "Z" => 'z',
        "Num0" | "Keypad0" => '0',
        "Num1" | "Keypad1" => '1',
        "Num2" | "Keypad2" => '2',
        "Num3" | "Keypad3" => '3',
        "Num4" | "Keypad4" => '4',
        "Num5" | "Keypad5" => '5',
        "Num6" | "Keypad6" => '6',
        "Num7" | "Keypad7" => '7',
        "Num8" | "Keypad8" => '8',
        "Num9" | "Keypad9" => '9',
        "Space" => ' ',
        "Minus" | "KeypadMinus" => '-',
        "Equal" | "KeypadEquals" => '=',
        "Comma" => ',',
        "Period" | "KeypadDecimal" => '.',
        "Slash" | "KeypadDivide" => '/',
        "LeftBracket" => '(',
        "RightBracket" => ')',
        _ => return None,
    };
    Some(if shifted { ch.to_ascii_uppercase() } else { ch })
}

fn column_label(col: usize) -> char {
    (b'A' + (col as u8).saturating_sub(1)) as char
}

fn run_native_app_window_example_into(
    name: &str,
    dir: &Path,
    smoke: &boon_backend_app_window::AppWindowSmoke,
    hold: Duration,
) -> Result<GateResult> {
    let mut app = app(name)?;
    let mut backend = WgpuBackend::headless_real(1280, 720)?;
    let initial = backend.load(&mut app)?;
    let native_script = run_native_scripted_scenario(name, &mut app, &mut backend)?;
    let timing = browser_timing_gate(name, &mut app, &mut backend)?;
    let frame = backend.capture_frame()?;
    let frame_png = write_wgpu_frame_png(&backend, dir, "frame.png")?;
    let replay = replay_wgpu(name, &app.snapshot(), frame.rgba_hash.as_deref())?;
    let source_inventory = app.source_inventory();
    let source_count = source_inventory.entries.len();
    fs::write(
        dir.join("timings.json"),
        serde_json::to_vec_pretty(&timing)?,
    )?;
    fs::write(dir.join("replay.json"), serde_json::to_vec_pretty(&replay)?)?;
    fs::write(
        dir.join("trace.json"),
        serde_json::to_vec_pretty(&json!({
            "example": name,
            "mode": "native-app-window",
            "scenario_builder": scenario_for_example(name),
            "initial_hash": initial.hash,
            "final_rgba_hash": frame.rgba_hash,
            "frame_png": &frame_png,
            "app_window": smoke,
            "wgpu_metadata": backend.metadata(),
            "source_inventory": source_inventory,
            "snapshot": app.snapshot(),
            "frame": frame,
            "scenario_steps": replay_steps(name),
            "human_like_interactions": human_like_interactions(name),
            "native_input_mapping": native_script,
            "manual_controls": native_manual_controls(name),
            "manual_preview_hold_ms": hold.as_millis(),
        }))?,
    )?;
    let timing_passed = timing
        .get("passed")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let passed = frame.rgba_hash.is_some()
        && frame.rgba_hash.as_deref() != Some("")
        && frame_png.nonblank
        && source_count > 0
        && timing_passed
        && native_script.passed
        && replay.passed;
    Ok(GateResult {
        backend: Backend::NativeAppWindow,
        example: name.to_string(),
        passed,
        frame_hash: frame.rgba_hash,
        artifact_dir: dir.to_path_buf(),
        message: if passed {
            "passed native app_window surface creation/present, synthetic human-like scenario dispatch, internal framebuffer readback, PNG frame artifact, timing evidence, source inventory, and replay gate".to_string()
        } else {
            "native app_window example scenario, timing, replay, source inventory, or frame hash gate failed".to_string()
        },
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct NativeScriptProof {
    passed: bool,
    actions: Vec<String>,
    batches: Vec<serde_json::Value>,
    snapshot_hash: String,
}

fn run_native_scripted_scenario(
    name: &str,
    app: &mut impl BoonApp,
    backend: &mut WgpuBackend,
) -> Result<NativeScriptProof> {
    let mut actions = Vec::new();
    let mut batches = Vec::new();
    macro_rules! dispatch {
        ($action:expr, $batch:expr $(,)?) => {{
            let batch = $batch;
            batches.push(serde_json::to_value(&batch)?);
            actions.push($action.to_string());
            backend.dispatch_frame_ready(app, batch)?;
            Ok::<(), anyhow::Error>(())
        }};
    }

    match name {
        "counter" | "counter_hold" => {
            for _ in 0..10 {
                dispatch!(
                    "click increment button",
                    event(
                        "store.sources.increment_button.event.press",
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            expect(app.snapshot().values.get("counter"), json!(10), "counter")?;
        }
        "interval" | "interval_hold" => {
            actions.push("advance clock by 3000ms".to_string());
            batches.push(json!("advance_fake_time 3000ms"));
            let result = app.advance_fake_time(Duration::from_secs(3));
            backend.apply_patches(&result.patches)?;
            backend.render_frame()?;
            expect(
                app.snapshot().values.get("interval_count"),
                json!(3),
                "interval_count",
            )?;
        }
        "todo_mvc" | "todo_mvc_physical" => {
            if name == "todo_mvc" {
                dispatch!(
                    "focus new todo input",
                    event(
                        "store.sources.new_todo_input.event.focus",
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            let mut typed = String::new();
            for ch in "Buy milk".chars() {
                typed.push(ch);
                dispatch!(
                    "type character into new todo input",
                    state(
                        "store.sources.new_todo_input.text",
                        SourceValue::Text(typed.clone()),
                    ),
                )?;
                dispatch!(
                    "emit new todo input change",
                    event(
                        "store.sources.new_todo_input.event.change",
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            dispatch!(
                "press Enter in new todo input",
                event(
                    "store.sources.new_todo_input.event.key_down.key",
                    SourceValue::Tag("Enter".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos_count"),
                json!(3),
                "store.todos_count",
            )?;
            dispatch!(
                "type whitespace-only todo text",
                state(
                    "store.sources.new_todo_input.text",
                    SourceValue::Text("   ".to_string()),
                ),
            )?;
            dispatch!(
                "press Enter to reject whitespace-only todo",
                event(
                    "store.sources.new_todo_input.event.key_down.key",
                    SourceValue::Tag("Enter".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos_count"),
                json!(3),
                "store.todos_count after whitespace-only input",
            )?;
            let mut edited = String::new();
            for ch in "Buy oat milk".chars() {
                edited.push(ch);
                dispatch!(
                    "type character into todo edit input",
                    dynamic_state(
                        "todos[*].sources.edit_input.text",
                        "3",
                        0,
                        SourceValue::Text(edited.clone()),
                    ),
                )?;
                dispatch!(
                    "emit todo edit change",
                    dynamic_event(
                        "todos[*].sources.edit_input.event.change",
                        "3",
                        0,
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            dispatch!(
                "press Enter in todo edit input",
                dynamic_event(
                    "todos[*].sources.edit_input.event.key_down.key",
                    "3",
                    0,
                    SourceValue::Tag("Enter".to_string()),
                ),
            )?;
            dispatch!(
                "blur todo edit input",
                dynamic_event(
                    "todos[*].sources.edit_input.event.blur",
                    "3",
                    0,
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos[3].title"),
                json!("Buy oat milk"),
                "store.todos[3].title after edit",
            )?;
            dispatch!(
                "click toggle all checkbox",
                event(
                    "store.sources.toggle_all_checkbox.event.click",
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.completed_todos_count"),
                json!(3),
                "store.completed_todos_count",
            )?;
            dispatch!(
                "click todo item checkbox",
                dynamic_event(
                    "todos[*].sources.checkbox.event.click",
                    "1",
                    0,
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.completed_todos_count"),
                json!(2),
                "store.completed_todos_count after item toggle",
            )?;
            if name == "todo_mvc" {
                for filter in ["completed", "active", "all"] {
                    dispatch!(
                        &format!("click {filter} filter"),
                        event(
                            &format!("store.sources.filter_{filter}.event.press"),
                            SourceValue::EmptyRecord,
                        ),
                    )?;
                }
            }
            dispatch!(
                "click todo item remove button",
                dynamic_event(
                    "todos[*].sources.remove_button.event.press",
                    "2",
                    0,
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos_count"),
                json!(2),
                "store.todos_count after item remove",
            )?;
            dispatch!(
                "click clear completed",
                event(
                    "store.sources.clear_completed_button.event.press",
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos_count"),
                json!(1),
                "store.todos_count after clear completed",
            )?;
            expect(
                app.snapshot().values.get("store.completed_todos_count"),
                json!(0),
                "store.completed_todos_count after clear completed",
            )?;
        }
        "cells" => {
            dispatch!(
                "double-click A1 display",
                dynamic_event(
                    "cells[*].sources.display.event.double_click",
                    "A1",
                    0,
                    SourceValue::EmptyRecord,
                ),
            )?;
            for (action, owner, text) in [
                ("type A1 plain value", "A1", "1"),
                ("type A2 plain value", "A2", "2"),
                ("type A3 plain value", "A3", "3"),
                ("type B1 formula", "B1", "=add(A1, A2)"),
                ("type B2 formula", "B2", "=sum(A1:A3)"),
                ("change A2 dependent value", "A2", "5"),
                ("type invalid A3 formula", "A3", "=bad("),
                ("type A1 cycle formula", "A1", "=add(A1, A2)"),
            ] {
                dispatch!(
                    action,
                    dynamic_state(
                        "cells[*].sources.editor.text",
                        owner,
                        0,
                        SourceValue::Text(text.to_string()),
                    ),
                )?;
            }
            dispatch!(
                "press Enter in cell editor",
                dynamic_event(
                    "cells[*].sources.editor.event.key_down.key",
                    "A1",
                    0,
                    SourceValue::Tag("Enter".to_string()),
                ),
            )?;
            for _ in 0..25 {
                dispatch!(
                    "press ArrowRight in grid viewport",
                    event(
                        "store.sources.viewport.event.key_down.key",
                        SourceValue::Tag("ArrowRight".to_string()),
                    ),
                )?;
            }
            for _ in 0..99 {
                dispatch!(
                    "press ArrowDown in grid viewport",
                    event(
                        "store.sources.viewport.event.key_down.key",
                        SourceValue::Tag("ArrowDown".to_string()),
                    ),
                )?;
            }
            expect(
                app.snapshot().values.get("cells.A1"),
                json!("#CYCLE"),
                "cells.A1 cycle formula",
            )?;
            expect(
                app.snapshot().values.get("cells.A3"),
                json!("#ERR"),
                "cells.A3 invalid formula",
            )?;
            expect(
                app.snapshot().values.get("cells.selected"),
                json!("Z100"),
                "cells.selected after viewport movement",
            )?;
        }
        "pong" | "arkanoid" => {
            for key in ["ArrowUp", "ArrowDown"] {
                dispatch!(
                    &format!("press {key} game control"),
                    event(
                        "store.sources.paddle.event.key_down.key",
                        SourceValue::Tag(key.to_string()),
                    ),
                )?;
            }
            dispatch!(
                "advance deterministic frame",
                event("store.sources.tick.event.frame", SourceValue::EmptyRecord),
            )?;
        }
        _ => bail!("unknown native app_window scripted scenario example `{name}`"),
    }

    Ok(NativeScriptProof {
        passed: true,
        actions,
        batches,
        snapshot_hash: snapshot_hash(&app.snapshot())?,
    })
}

fn run_core_scenario(
    name: &str,
    app: &mut impl BoonApp,
    backend: &mut RatatuiBackend,
) -> Result<()> {
    match name {
        "counter" | "counter_hold" => {
            for _ in 0..10 {
                backend.dispatch(
                    app,
                    event(
                        "store.sources.increment_button.event.press",
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            expect(app.snapshot().values.get("counter"), json!(10), "counter")?;
        }
        "interval" | "interval_hold" => {
            let result = app.advance_fake_time(Duration::from_secs(3));
            backend.apply_patches(&result.patches)?;
            expect(
                app.snapshot().values.get("interval_count"),
                json!(3),
                "interval_count",
            )?;
        }
        "todo_mvc" | "todo_mvc_physical" => {
            if name == "todo_mvc" {
                backend.dispatch(
                    app,
                    event(
                        "store.sources.new_todo_input.event.focus",
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            let mut typed = String::new();
            for ch in "Buy milk".chars() {
                typed.push(ch);
                backend.dispatch(
                    app,
                    state(
                        "store.sources.new_todo_input.text",
                        SourceValue::Text(typed.clone()),
                    ),
                )?;
                backend.dispatch(
                    app,
                    event(
                        "store.sources.new_todo_input.event.change",
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            backend.dispatch(
                app,
                event(
                    "store.sources.new_todo_input.event.key_down.key",
                    SourceValue::Tag("Enter".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos_count"),
                json!(3),
                "store.todos_count",
            )?;
            backend.dispatch(
                app,
                state(
                    "store.sources.new_todo_input.text",
                    SourceValue::Text("   ".to_string()),
                ),
            )?;
            backend.dispatch(
                app,
                event(
                    "store.sources.new_todo_input.event.key_down.key",
                    SourceValue::Tag("Enter".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos_count"),
                json!(3),
                "store.todos_count after whitespace-only input",
            )?;
            let mut edited = String::new();
            for ch in "Buy oat milk".chars() {
                edited.push(ch);
                backend.dispatch(
                    app,
                    dynamic_state(
                        "todos[*].sources.edit_input.text",
                        "3",
                        0,
                        SourceValue::Text(edited.clone()),
                    ),
                )?;
                backend.dispatch(
                    app,
                    dynamic_event(
                        "todos[*].sources.edit_input.event.change",
                        "3",
                        0,
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            backend.dispatch(
                app,
                dynamic_event(
                    "todos[*].sources.edit_input.event.key_down.key",
                    "3",
                    0,
                    SourceValue::Tag("Enter".to_string()),
                ),
            )?;
            backend.dispatch(
                app,
                dynamic_event(
                    "todos[*].sources.edit_input.event.blur",
                    "3",
                    0,
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos[3].title"),
                json!("Buy oat milk"),
                "store.todos[3].title after edit",
            )?;
            backend.dispatch(
                app,
                event(
                    "store.sources.toggle_all_checkbox.event.click",
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.completed_todos_count"),
                json!(3),
                "store.completed_todos_count",
            )?;
            backend.dispatch(
                app,
                dynamic_event(
                    "todos[*].sources.checkbox.event.click",
                    "1",
                    0,
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.completed_todos_count"),
                json!(2),
                "store.completed_todos_count after item toggle",
            )?;
            if name == "todo_mvc" {
                for filter in ["completed", "active", "all"] {
                    backend.dispatch(
                        app,
                        event(
                            &format!("store.sources.filter_{filter}.event.press"),
                            SourceValue::EmptyRecord,
                        ),
                    )?;
                }
            }
            backend.dispatch(
                app,
                dynamic_event(
                    "todos[*].sources.remove_button.event.press",
                    "2",
                    0,
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos_count"),
                json!(2),
                "store.todos_count after item remove",
            )?;
            backend.dispatch(
                app,
                event(
                    "store.sources.clear_completed_button.event.press",
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos_count"),
                json!(1),
                "store.todos_count after clear completed",
            )?;
            expect(
                app.snapshot().values.get("store.completed_todos_count"),
                json!(0),
                "store.completed_todos_count after clear completed",
            )?;
        }
        "cells" => {
            backend.dispatch(
                app,
                dynamic_event(
                    "cells[*].sources.display.event.double_click",
                    "A1",
                    0,
                    SourceValue::EmptyRecord,
                ),
            )?;
            backend.dispatch(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A1",
                    0,
                    SourceValue::Text("1".to_string()),
                ),
            )?;
            backend.dispatch(
                app,
                dynamic_event(
                    "cells[*].sources.editor.event.key_down.key",
                    "A1",
                    0,
                    SourceValue::Tag("Enter".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("cells.A1"),
                json!("1"),
                "cells.A1",
            )?;
            for (owner, text) in [("A2", "2"), ("A3", "3")] {
                backend.dispatch(
                    app,
                    dynamic_state(
                        "cells[*].sources.editor.text",
                        owner,
                        0,
                        SourceValue::Text(text.to_string()),
                    ),
                )?;
                backend.dispatch(
                    app,
                    dynamic_event(
                        "cells[*].sources.editor.event.change",
                        owner,
                        0,
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            for (owner, text) in [("B1", "=add(A1, A2)"), ("B2", "=sum(A1:A3)")] {
                backend.dispatch(
                    app,
                    dynamic_state(
                        "cells[*].sources.editor.text",
                        owner,
                        0,
                        SourceValue::Text(text.to_string()),
                    ),
                )?;
            }
            expect(
                app.snapshot().values.get("cells.B1"),
                json!("3"),
                "cells.B1 after formula",
            )?;
            expect(
                app.snapshot().values.get("cells.B2"),
                json!("6"),
                "cells.B2 after sum",
            )?;
            backend.dispatch(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A2",
                    0,
                    SourceValue::Text("5".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("cells.B1"),
                json!("6"),
                "cells.B1 after A2 change",
            )?;
            expect(
                app.snapshot().values.get("cells.B2"),
                json!("9"),
                "cells.B2 after A2 change",
            )?;
            backend.dispatch(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A3",
                    0,
                    SourceValue::Text("=bad(".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("cells.A3"),
                json!("#ERR"),
                "cells.A3 invalid formula",
            )?;
            backend.dispatch(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A1",
                    0,
                    SourceValue::Text("=add(A1, A2)".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("cells.A1"),
                json!("#CYCLE"),
                "cells.A1 cycle formula",
            )?;
            for _ in 0..25 {
                backend.dispatch(
                    app,
                    event(
                        "store.sources.viewport.event.key_down.key",
                        SourceValue::Tag("ArrowRight".to_string()),
                    ),
                )?;
            }
            for _ in 0..99 {
                backend.dispatch(
                    app,
                    event(
                        "store.sources.viewport.event.key_down.key",
                        SourceValue::Tag("ArrowDown".to_string()),
                    ),
                )?;
            }
            expect(
                app.snapshot().values.get("cells.selected"),
                json!("Z100"),
                "cells.selected after viewport movement",
            )?;
        }
        "pong" | "arkanoid" => {
            for key in ["ArrowUp", "ArrowDown"] {
                backend.dispatch(
                    app,
                    event(
                        "store.sources.paddle.event.key_down.key",
                        SourceValue::Tag(key.to_string()),
                    ),
                )?;
            }
            backend.dispatch(
                app,
                event("store.sources.tick.event.frame", SourceValue::EmptyRecord),
            )?;
        }
        _ => bail!("unknown scenario example `{name}`"),
    }
    Ok(())
}

fn run_core_scenario_wgpu(
    name: &str,
    app: &mut impl BoonApp,
    backend: &mut WgpuBackend,
) -> Result<()> {
    match name {
        "counter" | "counter_hold" => {
            for _ in 0..10 {
                backend.dispatch_frame_ready(
                    app,
                    event(
                        "store.sources.increment_button.event.press",
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            expect(app.snapshot().values.get("counter"), json!(10), "counter")?;
        }
        "interval" | "interval_hold" => {
            let result = app.advance_fake_time(Duration::from_secs(3));
            backend.apply_patches(&result.patches)?;
            backend.render_frame()?;
            expect(
                app.snapshot().values.get("interval_count"),
                json!(3),
                "interval_count",
            )?;
        }
        "todo_mvc" | "todo_mvc_physical" => {
            if name == "todo_mvc" {
                backend.dispatch_frame_ready(
                    app,
                    event(
                        "store.sources.new_todo_input.event.focus",
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            let mut typed = String::new();
            for ch in "Buy milk".chars() {
                typed.push(ch);
                backend.dispatch_frame_ready(
                    app,
                    state(
                        "store.sources.new_todo_input.text",
                        SourceValue::Text(typed.clone()),
                    ),
                )?;
                backend.dispatch_frame_ready(
                    app,
                    event(
                        "store.sources.new_todo_input.event.change",
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            backend.dispatch_frame_ready(
                app,
                event(
                    "store.sources.new_todo_input.event.key_down.key",
                    SourceValue::Tag("Enter".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos_count"),
                json!(3),
                "store.todos_count",
            )?;
            backend.dispatch_frame_ready(
                app,
                state(
                    "store.sources.new_todo_input.text",
                    SourceValue::Text("   ".to_string()),
                ),
            )?;
            backend.dispatch_frame_ready(
                app,
                event(
                    "store.sources.new_todo_input.event.key_down.key",
                    SourceValue::Tag("Enter".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos_count"),
                json!(3),
                "store.todos_count after whitespace-only input",
            )?;
            let mut edited = String::new();
            for ch in "Buy oat milk".chars() {
                edited.push(ch);
                backend.dispatch_frame_ready(
                    app,
                    dynamic_state(
                        "todos[*].sources.edit_input.text",
                        "3",
                        0,
                        SourceValue::Text(edited.clone()),
                    ),
                )?;
                backend.dispatch_frame_ready(
                    app,
                    dynamic_event(
                        "todos[*].sources.edit_input.event.change",
                        "3",
                        0,
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            backend.dispatch_frame_ready(
                app,
                dynamic_event(
                    "todos[*].sources.edit_input.event.key_down.key",
                    "3",
                    0,
                    SourceValue::Tag("Enter".to_string()),
                ),
            )?;
            backend.dispatch_frame_ready(
                app,
                dynamic_event(
                    "todos[*].sources.edit_input.event.blur",
                    "3",
                    0,
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos[3].title"),
                json!("Buy oat milk"),
                "store.todos[3].title after edit",
            )?;
            backend.dispatch_frame_ready(
                app,
                event(
                    "store.sources.toggle_all_checkbox.event.click",
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.completed_todos_count"),
                json!(3),
                "store.completed_todos_count",
            )?;
            backend.dispatch_frame_ready(
                app,
                dynamic_event(
                    "todos[*].sources.checkbox.event.click",
                    "1",
                    0,
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.completed_todos_count"),
                json!(2),
                "store.completed_todos_count after item toggle",
            )?;
            if name == "todo_mvc" {
                for filter in ["completed", "active", "all"] {
                    backend.dispatch_frame_ready(
                        app,
                        event(
                            &format!("store.sources.filter_{filter}.event.press"),
                            SourceValue::EmptyRecord,
                        ),
                    )?;
                }
            }
            backend.dispatch_frame_ready(
                app,
                dynamic_event(
                    "todos[*].sources.remove_button.event.press",
                    "2",
                    0,
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos_count"),
                json!(2),
                "store.todos_count after item remove",
            )?;
            backend.dispatch_frame_ready(
                app,
                event(
                    "store.sources.clear_completed_button.event.press",
                    SourceValue::EmptyRecord,
                ),
            )?;
            expect(
                app.snapshot().values.get("store.todos_count"),
                json!(1),
                "store.todos_count after clear completed",
            )?;
            expect(
                app.snapshot().values.get("store.completed_todos_count"),
                json!(0),
                "store.completed_todos_count after clear completed",
            )?;
        }
        "cells" => {
            backend.dispatch_frame_ready(
                app,
                dynamic_event(
                    "cells[*].sources.display.event.double_click",
                    "A1",
                    0,
                    SourceValue::EmptyRecord,
                ),
            )?;
            backend.dispatch_frame_ready(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A1",
                    0,
                    SourceValue::Text("1".to_string()),
                ),
            )?;
            backend.dispatch_frame_ready(
                app,
                dynamic_event(
                    "cells[*].sources.editor.event.key_down.key",
                    "A1",
                    0,
                    SourceValue::Tag("Enter".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("cells.A1"),
                json!("1"),
                "cells.A1",
            )?;
            for (owner, text) in [("A2", "2"), ("A3", "3")] {
                backend.dispatch_frame_ready(
                    app,
                    dynamic_state(
                        "cells[*].sources.editor.text",
                        owner,
                        0,
                        SourceValue::Text(text.to_string()),
                    ),
                )?;
                backend.dispatch_frame_ready(
                    app,
                    dynamic_event(
                        "cells[*].sources.editor.event.change",
                        owner,
                        0,
                        SourceValue::EmptyRecord,
                    ),
                )?;
            }
            for (owner, text) in [("B1", "=add(A1, A2)"), ("B2", "=sum(A1:A3)")] {
                backend.dispatch_frame_ready(
                    app,
                    dynamic_state(
                        "cells[*].sources.editor.text",
                        owner,
                        0,
                        SourceValue::Text(text.to_string()),
                    ),
                )?;
            }
            expect(
                app.snapshot().values.get("cells.B1"),
                json!("3"),
                "cells.B1 after formula",
            )?;
            expect(
                app.snapshot().values.get("cells.B2"),
                json!("6"),
                "cells.B2 after sum",
            )?;
            backend.dispatch_frame_ready(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A2",
                    0,
                    SourceValue::Text("5".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("cells.B1"),
                json!("6"),
                "cells.B1 after A2 change",
            )?;
            expect(
                app.snapshot().values.get("cells.B2"),
                json!("9"),
                "cells.B2 after A2 change",
            )?;
            backend.dispatch_frame_ready(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A3",
                    0,
                    SourceValue::Text("=bad(".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("cells.A3"),
                json!("#ERR"),
                "cells.A3 invalid formula",
            )?;
            backend.dispatch_frame_ready(
                app,
                dynamic_state(
                    "cells[*].sources.editor.text",
                    "A1",
                    0,
                    SourceValue::Text("=add(A1, A2)".to_string()),
                ),
            )?;
            expect(
                app.snapshot().values.get("cells.A1"),
                json!("#CYCLE"),
                "cells.A1 cycle formula",
            )?;
            for _ in 0..25 {
                backend.dispatch_frame_ready(
                    app,
                    event(
                        "store.sources.viewport.event.key_down.key",
                        SourceValue::Tag("ArrowRight".to_string()),
                    ),
                )?;
            }
            for _ in 0..99 {
                backend.dispatch_frame_ready(
                    app,
                    event(
                        "store.sources.viewport.event.key_down.key",
                        SourceValue::Tag("ArrowDown".to_string()),
                    ),
                )?;
            }
            expect(
                app.snapshot().values.get("cells.selected"),
                json!("Z100"),
                "cells.selected after viewport movement",
            )?;
        }
        "pong" | "arkanoid" => {
            for key in ["ArrowUp", "ArrowDown"] {
                backend.dispatch_frame_ready(
                    app,
                    event(
                        "store.sources.paddle.event.key_down.key",
                        SourceValue::Tag(key.to_string()),
                    ),
                )?;
            }
            backend.dispatch_frame_ready(
                app,
                event("store.sources.tick.event.frame", SourceValue::EmptyRecord),
            )?;
        }
        _ => bail!("unknown scenario example `{name}`"),
    }
    Ok(())
}

fn event(path: &str, value: SourceValue) -> SourceBatch {
    SourceBatch {
        state_updates: Vec::new(),
        events: vec![SourceEmission {
            path: path.to_string(),
            value,
            owner_id: None,
            owner_generation: None,
        }],
    }
}

fn state(path: &str, value: SourceValue) -> SourceBatch {
    SourceBatch {
        state_updates: vec![SourceEmission {
            path: path.to_string(),
            value,
            owner_id: None,
            owner_generation: None,
        }],
        events: Vec::new(),
    }
}

fn dynamic_event(
    path: &str,
    owner_id: &str,
    owner_generation: u32,
    value: SourceValue,
) -> SourceBatch {
    SourceBatch {
        state_updates: Vec::new(),
        events: vec![SourceEmission {
            path: path.to_string(),
            value,
            owner_id: Some(owner_id.to_string()),
            owner_generation: Some(owner_generation),
        }],
    }
}

fn dynamic_state(
    path: &str,
    owner_id: &str,
    owner_generation: u32,
    value: SourceValue,
) -> SourceBatch {
    SourceBatch {
        state_updates: vec![SourceEmission {
            path: path.to_string(),
            value,
            owner_id: Some(owner_id.to_string()),
            owner_generation: Some(owner_generation),
        }],
        events: Vec::new(),
    }
}

fn expect(
    actual: Option<&serde_json::Value>,
    expected: serde_json::Value,
    label: &str,
) -> Result<()> {
    if actual != Some(&expected) {
        bail!("expected {label} to be {expected}, got {actual:?}");
    }
    Ok(())
}

fn stable_sha(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}
