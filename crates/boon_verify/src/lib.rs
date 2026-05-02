use anyhow::{Context, Result, bail};
use boon_backend_app_window::{
    AppWindowCloseProof, AppWindowInputSample, AppWindowSurfaceFrameProof, RgbaFrame,
    run_close_probe, run_rgba_input_session, run_rgba_input_session_with_proof,
    smoke_test_with_title as app_window_smoke_test_with_title,
};
use boon_backend_browser::BrowserScenarioInput;
use boon_backend_ratatui::RatatuiBackend;
use boon_backend_wgpu::{FrameImageArtifact, WgpuBackend, hash_rgba, rasterize_native_gui_frame};
use boon_examples::{app, list_examples};
use boon_runtime::{BoonApp, SourceBatch, SourceEmission, SourceValue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufReader, BufWriter, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine as _;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Backend {
    QualityGate,
    BoonPowered,
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
pub struct BoonPoweredGateReport {
    pub passed: bool,
    pub scanned_files: Vec<String>,
    pub violations: Vec<BoonPoweredViolation>,
    pub mutation_probes: Vec<BoonPoweredMutationProbe>,
    pub generated_provenance: BoonPoweredGeneratedProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BoonPoweredViolation {
    pub check: String,
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub evidence: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BoonPoweredMutationProbe {
    pub example: String,
    pub mutation: String,
    pub original_compile_ok: bool,
    pub mutated_compile_ok: bool,
    pub changed_compiled_output: bool,
    pub original_sha256: Option<String>,
    pub mutated_sha256: Option<String>,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BoonPoweredGeneratedProvenance {
    pub path: String,
    pub exists: bool,
    pub required: bool,
    pub has_source_sha256: bool,
    pub has_ir_sha256: bool,
    pub has_source_span_table: bool,
    pub passed: bool,
}

fn prepare_artifact_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir).with_context(|| {
            format!("failed to clear stale artifact directory {}", dir.display())
        })?;
    }
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create artifact directory {}", dir.display()))?;
    Ok(())
}

pub fn verify_boon_powered(artifacts: &Path) -> Result<VerifyReport> {
    let root = repo_root()?;
    let dir = artifacts.join("boon-powered");
    prepare_artifact_dir(&dir)?;
    let report = boon_powered_gate_report(&root, true)?;
    let report_path = dir.join("boon-powered-gate.json");
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    Ok(VerifyReport {
        command: "verify boon-powered".to_string(),
        results: vec![GateResult {
            backend: Backend::BoonPowered,
            example: "all".to_string(),
            passed: report.passed,
            frame_hash: None,
            artifact_dir: dir,
            message: if report.passed {
                "passed Boon-powered anti-cheat gate".to_string()
            } else {
                format!(
                    "Boon-powered anti-cheat gate failed: {} handwritten Rust violations, {} failed mutation/provenance checks; see {}",
                    report.violations.len(),
                    report
                        .mutation_probes
                        .iter()
                        .filter(|probe| !probe.passed)
                        .count()
                        + usize::from(!report.generated_provenance.passed),
                    report_path.display()
                )
            },
        }],
    })
}

pub fn boon_powered_gate_report(
    root: &Path,
    require_generated: bool,
) -> Result<BoonPoweredGateReport> {
    let scanned_files = handwritten_rust_files(root)?;
    let mut violations = Vec::new();
    for rel in &scanned_files {
        scan_boon_powered_file(root, rel, &mut violations)?;
    }
    let mutation_probes = source_mutation_probes(root)?;
    let generated_provenance = generated_provenance(root, require_generated)?;
    let passed = violations.is_empty()
        && mutation_probes.iter().all(|probe| probe.passed)
        && generated_provenance.passed;
    Ok(BoonPoweredGateReport {
        passed,
        scanned_files,
        violations,
        mutation_probes,
        generated_provenance,
    })
}

fn repo_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("boon_verify crate is expected under <repo>/crates/boon_verify")
}

fn handwritten_rust_files(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for rel in [
        "crates/boon_compiler/src",
        "crates/boon_runtime/src",
        "crates/boon_codegen_rust/src",
        "crates/boon_backend_wgpu/src",
        "crates/boon_backend_ratatui/src",
    ] {
        collect_rust_files(root, Path::new(rel), &mut files)?;
    }
    files.sort();
    Ok(files)
}

fn collect_rust_files(root: &Path, rel: &Path, files: &mut Vec<String>) -> Result<()> {
    let dir = root.join(rel);
    for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let rel_path = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            collect_rust_files(root, Path::new(&rel_path), files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(rel_path);
        }
    }
    Ok(())
}

fn scan_boon_powered_file(
    root: &Path,
    rel: &str,
    violations: &mut Vec<BoonPoweredViolation>,
) -> Result<()> {
    let path = root.join(rel);
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let deny = boon_powered_forbidden_needles();
    for (line_index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        for (check, needle) in &deny {
            if let Some(column) = line.find(needle) {
                violations.push(BoonPoweredViolation {
                    check: (*check).to_string(),
                    path: rel.to_string(),
                    line: line_index + 1,
                    column: column + 1,
                    evidence: line.trim().to_string(),
                });
            }
        }
    }
    Ok(())
}

fn boon_powered_forbidden_needles() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "example/domain model in handwritten Rust",
            "CollectionBinding",
        ),
        ("example/domain model in handwritten Rust", "TableBinding"),
        (
            "example/domain model in handwritten Rust",
            "FormulaTableState",
        ),
        ("example/domain model in handwritten Rust", "PlayfieldState"),
        ("example/domain model in handwritten Rust", "CollectionSpec"),
        ("example/domain model in handwritten Rust", "TableSpec"),
        ("example/domain model in handwritten Rust", "PlayfieldSpec"),
        ("example/domain model in handwritten Rust", "PaddleSpec"),
        ("example/domain model in handwritten Rust", "BrickFieldSpec"),
        (
            "example/domain model in handwritten Rust",
            "SurfaceKind::Collection",
        ),
        (
            "example/domain model in handwritten Rust",
            "SurfaceKind::Table",
        ),
        (
            "example/domain model in handwritten Rust",
            "SurfaceKind::Playfield",
        ),
        (
            "handcrafted renderer in handwritten Rust",
            "render_collection_scene",
        ),
        (
            "handcrafted renderer in handwritten Rust",
            "render_table_scene",
        ),
        (
            "handcrafted renderer in handwritten Rust",
            "render_playfield_scene",
        ),
        ("handcrafted renderer in handwritten Rust", "draw_todomvc"),
        ("handcrafted renderer in handwritten Rust", "draw_cells"),
        ("handcrafted renderer in handwritten Rust", "draw_game"),
        (
            "source text heuristic in compiler/runtime",
            "source.contains(",
        ),
        ("source text heuristic in compiler/runtime", "source.find("),
        ("source text heuristic in compiler/runtime", "named_block("),
        (
            "source text heuristic in compiler/runtime",
            "extract_initial_collection_titles",
        ),
        (
            "source text heuristic in compiler/runtime",
            "extract_formula_functions",
        ),
        (
            "source text heuristic in compiler/runtime",
            "extract_hold_increment",
        ),
        (
            "source text heuristic in compiler/runtime",
            "extract_text_record_field",
        ),
        (
            "source text heuristic in compiler/runtime",
            "static_view_names",
        ),
        (
            "source text heuristic in compiler/runtime",
            "playfield_spec(",
        ),
        ("source text heuristic in compiler/runtime", "paddle_spec("),
        ("todo business logic in handwritten Rust", "new_todo"),
        ("todo business logic in handwritten Rust", "toggle_all"),
        ("todo business logic in handwritten Rust", "clear_completed"),
        ("todo business logic in handwritten Rust", "selected_filter"),
        ("todo business logic in handwritten Rust", "completed_todos"),
        ("todo business logic in handwritten Rust", "active_todos"),
        ("cells business logic in handwritten Rust", "formula"),
        ("cells business logic in handwritten Rust", "Formula"),
        ("cells business logic in handwritten Rust", "grid_text"),
        ("game business logic in handwritten Rust", "paddle"),
        ("game business logic in handwritten Rust", "Paddle"),
        ("game business logic in handwritten Rust", "brick"),
        ("game business logic in handwritten Rust", "Brick"),
        ("game business logic in handwritten Rust", "Arkanoid"),
        ("game business logic in handwritten Rust", "Pong"),
        ("example-name branch in handwritten Rust", "todo_mvc"),
        ("example-name branch in handwritten Rust", "arkanoid"),
    ]
}

fn source_mutation_probes(root: &Path) -> Result<Vec<BoonPoweredMutationProbe>> {
    let probes = [
        ("counter", "state + 1", "state + 2"),
        ("counter_hold", "state + 1", "state + 2"),
        ("interval", "state + 1", "state + 2"),
        ("interval_hold", "state + 1", "state + 2"),
        ("todo_mvc", "List/append", "List/append_broken"),
        ("todo_mvc_physical", "List/append", "List/append_broken"),
        ("cells", "Math/sum", "Math/sum_broken"),
        ("pong", "dx: -12", "dx: -13"),
        ("arkanoid", "columns: 12", "columns: 11"),
    ];
    probes
        .iter()
        .map(|(example, needle, replacement)| {
            source_mutation_probe(root, example, needle, replacement)
        })
        .collect()
}

fn source_mutation_probe(
    root: &Path,
    example: &str,
    needle: &str,
    replacement: &str,
) -> Result<BoonPoweredMutationProbe> {
    let path = root.join("examples").join(example).join("source.bn");
    let source =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let Some(_) = source.find(needle) else {
        return Ok(BoonPoweredMutationProbe {
            example: example.to_string(),
            mutation: format!("replace `{needle}` with `{replacement}`"),
            original_compile_ok: false,
            mutated_compile_ok: false,
            changed_compiled_output: false,
            original_sha256: None,
            mutated_sha256: None,
            passed: false,
            detail: format!("source did not contain mutation needle `{needle}`"),
        });
    };
    let mutated = source.replacen(needle, replacement, 1);
    let original = compile_and_hash(example, &source);
    let mutated_result = compile_and_hash(example, &mutated);
    let changed = match (&original, &mutated_result) {
        (Ok(original_hash), Ok(mutated_hash)) => original_hash != mutated_hash,
        (Ok(_), Err(_)) => true,
        _ => false,
    };
    Ok(BoonPoweredMutationProbe {
        example: example.to_string(),
        mutation: format!("replace `{needle}` with `{replacement}`"),
        original_compile_ok: original.is_ok(),
        mutated_compile_ok: mutated_result.is_ok(),
        changed_compiled_output: changed,
        original_sha256: original.as_ref().ok().cloned(),
        mutated_sha256: mutated_result.as_ref().ok().cloned(),
        passed: original.is_ok() && changed,
        detail: match (&original, &mutated_result) {
            (Ok(_), Ok(_)) if changed => {
                "mutated source compiled to a different Boon output".to_string()
            }
            (Ok(_), Err(err)) => format!("mutated source failed to compile: {err}"),
            (Ok(_), Ok(_)) => "mutated source compiled to identical Boon output".to_string(),
            (Err(err), _) => format!("original source failed to compile: {err}"),
        },
    })
}

fn compile_and_hash(example: &str, source: &str) -> Result<String> {
    let compiled = boon_compiler::compile_source(example, source)?;
    let bytes = serde_json::to_vec(&compiled)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn generated_provenance(root: &Path, required: bool) -> Result<BoonPoweredGeneratedProvenance> {
    let path = root.join("target/generated-examples/generated_examples.rs");
    let exists = path.exists();
    let text = if exists {
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };
    let has_source_sha256 = text.contains("SOURCE_SHA256");
    let has_ir_sha256 = text.contains("IR_SHA256");
    let has_source_span_table = text.contains("SOURCE_SPANS") || text.contains("SourceSpan");
    let passed = if required {
        exists && has_source_sha256 && has_ir_sha256 && has_source_span_table
    } else {
        !exists || (has_source_sha256 && has_ir_sha256 && has_source_span_table)
    };
    Ok(BoonPoweredGeneratedProvenance {
        path: path.to_string_lossy().to_string(),
        exists,
        required,
        has_source_sha256,
        has_ir_sha256,
        has_source_span_table,
        passed,
    })
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
        prepare_artifact_dir(&dir)?;
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

fn replay_native_app_window(
    name: &str,
    expected_snapshot: &boon_runtime::AppSnapshot,
    expected_frame_hash: Option<&str>,
) -> Result<ReplayProof> {
    let mut app = app(name)?;
    let mut backend = WgpuBackend::headless_real(1280, 720)?;
    backend.load(&mut app)?;
    let native_script = run_native_scripted_scenario(name, &mut app, &mut backend)?;
    let _ = browser_timing_gate(name, &mut app, &mut backend)?;
    let frame = backend.capture_frame()?;
    let expected_snapshot_hash = snapshot_hash(expected_snapshot)?;
    let replay_snapshot_hash = snapshot_hash(&app.snapshot())?;
    let mut steps = native_script.actions;
    steps.extend(replay_steps(name).into_iter().filter(|step| {
        step.starts_with("timing scenario ")
            || step == "mount"
            || step == "expect visible output"
            || step == "expect source inventory"
            || step == "expect replay"
            || step == "expect frame hash"
    }));
    Ok(ReplayProof {
        passed: expected_snapshot_hash == replay_snapshot_hash
            && expected_frame_hash == frame.rgba_hash.as_deref(),
        snapshot_hash: expected_snapshot_hash,
        replay_snapshot_hash,
        frame_hash: expected_frame_hash.map(str::to_string),
        replay_frame_hash: frame.rgba_hash,
        steps,
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
        prepare_artifact_dir(&dir)?;
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
    let boon_powered = verify_boon_powered(artifacts)?;
    let failed = boon_powered.results.iter().any(|r| !r.passed);
    results.extend(boon_powered.results);
    if failed {
        return Ok(VerifyReport {
            command: "verify all".to_string(),
            results,
        });
    }
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
    prepare_artifact_dir(&root_dir)?;
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
                prepare_artifact_dir(&dir)?;
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
                Some(16.0),
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
                Some(16.0),
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
    prepare_artifact_dir(&root_dir)?;
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
        prepare_artifact_dir(&dir)?;
        let surface_frame = match run_native_visible_surface_probe(name, &dir) {
            Ok(surface_frame) => surface_frame,
            Err(err) => {
                fs::write(dir.join("visible-surface-failure.txt"), err.to_string())?;
                results.push(GateResult {
                    backend: Backend::NativeAppWindow,
                    example: (*name).to_string(),
                    passed: false,
                    frame_hash: None,
                    artifact_dir: dir,
                    message: format!("native app_window visible surface proof failed: {err}"),
                });
                break;
            }
        };
        if let Err(err) = run_native_close_probe(name, &dir) {
            fs::write(dir.join("close-button-failure.txt"), err.to_string())?;
            results.push(GateResult {
                backend: Backend::NativeAppWindow,
                example: (*name).to_string(),
                passed: false,
                frame_hash: None,
                artifact_dir: dir,
                message: format!("native app_window close-button proof failed: {err}"),
            });
            break;
        }
        match run_native_app_window_example_into(
            name,
            &dir,
            &smoke,
            Duration::ZERO,
            Some(&surface_frame),
        ) {
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
    prepare_artifact_dir(&dir)?;
    let mut surface_frame = None;
    let smoke = if hold.is_zero() {
        app_window_smoke_test_with_title(format!("Boon {example} native app_window"), hold)?
    } else {
        let proof = run_native_manual_input_session(example, &dir, hold)?;
        surface_frame = proof.surface_frame.clone();
        fs::write(
            dir.join("manual-input.json"),
            serde_json::to_vec_pretty(&proof)?,
        )?;
        proof.app_window
    };
    run_native_app_window_example_into(example, &dir, &smoke, hold, surface_frame.as_ref())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct NativeManualInputProof {
    app_window: boon_backend_app_window::AppWindowSmoke,
    surface_frame: Option<AppWindowSurfaceFrameProof>,
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
    last_auto_tick: Instant,
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
            last_auto_tick: Instant::now(),
        })
    }

    fn new_for_playground(example: &str) -> Result<Self> {
        let mut app = app(example)?;
        let mut backend = WgpuBackend::headless(1280, 720);
        let turn = app.mount();
        backend.apply_patches(&turn.patches)?;
        backend.render_frame_ready()?;
        Ok(Self {
            example: example.to_string(),
            app,
            backend,
            focused: None,
            text_buffer: String::new(),
            samples_seen: 0,
            dispatches: Vec::new(),
            errors: Vec::new(),
            last_auto_tick: Instant::now(),
        })
    }

    fn handle_sample(&mut self, sample: AppWindowInputSample) -> Result<()> {
        self.samples_seen += 1;
        if sample.left_clicked && sample.mouse_x.is_some() && sample.mouse_y.is_some() {
            self.handle_click(&sample)?;
        }
        for key in &sample.newly_pressed_keys {
            self.handle_key(key, &sample.pressed_keys)?;
        }
        for key in &sample.repeated_keys {
            self.handle_key(key, &sample.pressed_keys)?;
        }
        Ok(())
    }

    fn handle_click(&mut self, sample: &AppWindowInputSample) -> Result<()> {
        let layout = NativeGuiLayout::from_sample(sample);
        let Some((x, y)) = layout.preview_virtual(sample) else {
            return Ok(());
        };
        match self.example.as_str() {
            "counter" | "counter_hold" => {
                if (338.0..=662.0).contains(&x) && (388.0..=480.0).contains(&y) {
                    self.dispatch_labeled(
                        "native mouse click increment button",
                        event(
                            "store.sources.increment_button.event.press",
                            SourceValue::EmptyRecord,
                        ),
                    )
                } else {
                    Ok(())
                }
            }
            "interval" | "interval_hold" => Ok(()),
            "todo_mvc" | "todo_mvc_physical" => self.handle_todo_click(x, y),
            "cells" => self.handle_cells_click(x, y),
            "pong" | "arkanoid" => Ok(()),
            _ => Ok(()),
        }
    }

    fn handle_todo_click(&mut self, x: f64, y: f64) -> Result<()> {
        let panel_x = 206.0;
        let main_y = 160.0;
        let panel_w = 588.0;
        if (main_y..main_y + 72.0).contains(&y) && (panel_x + 54.0..=panel_x + panel_w).contains(&x)
        {
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
        if (main_y..main_y + 72.0).contains(&y)
            && (panel_x..panel_x + 54.0).contains(&x)
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
        let visible_ids = self.visible_todo_ids();
        let row_h = 62.0;
        let footer_y = main_y + 72.0 + visible_ids.len() as f64 * row_h;
        if (footer_y..footer_y + 54.0).contains(&y) {
            if (panel_x + 474.0..=panel_x + panel_w).contains(&x)
                && self.completed_todo_count() > 0
                && self.has_source("store.sources.clear_completed_button.event.press")
            {
                return self.dispatch_labeled(
                    "native mouse click clear completed",
                    event(
                        "store.sources.clear_completed_button.event.press",
                        SourceValue::EmptyRecord,
                    ),
                );
            }
            let filter = if (panel_x + 124.0..panel_x + 214.0).contains(&x) {
                "all"
            } else if (panel_x + 214.0..panel_x + 319.0).contains(&x) {
                "active"
            } else if (panel_x + 319.0..panel_x + 459.0).contains(&x) {
                "completed"
            } else {
                return Ok(());
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
        if y < main_y + 72.0 {
            return Ok(());
        }
        let row = ((y - (main_y + 72.0)) / row_h).floor() as usize;
        let Some(owner_id) = visible_ids.get(row).cloned() else {
            return Ok(());
        };
        if (panel_x..panel_x + 58.0).contains(&x)
            && self.has_source("todos[*].sources.checkbox.event.click")
        {
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
        if x >= panel_x + panel_w - 58.0
            && self.has_source("todos[*].sources.remove_button.event.press")
        {
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
        let col = ((x - 100.0) / 92.0).floor() as i64 + 1;
        let row = ((y - 200.0) / 38.0).floor() as i64 + 1;
        if !(1..=26).contains(&col) || !(1..=100).contains(&row) {
            return Ok(());
        }
        let owner_id = format!("{}{}", column_label(col as usize), row);
        self.focused = Some(NativeFocus::Cell {
            owner_id: owner_id.clone(),
        });
        self.text_buffer.clear();
        if let Some(text) = self
            .app
            .snapshot()
            .values
            .get(&format!("cells.{owner_id}.formula"))
            .and_then(|value| value.as_str())
        {
            self.text_buffer = text.to_string();
        }
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

    fn advance_live_frame(&mut self) -> Result<()> {
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_auto_tick);
        let tick = match self.example.as_str() {
            "interval" | "interval_hold" => Duration::from_millis(250),
            "pong" | "arkanoid" => Duration::from_millis(50),
            _ => return Ok(()),
        };
        if elapsed < tick {
            return Ok(());
        }
        self.last_auto_tick = now;
        match self.example.as_str() {
            "interval" | "interval_hold" => {
                let result = self.app.advance_fake_time(elapsed);
                self.backend.apply_patches(&result.patches)?;
                self.backend.render_frame_ready()?;
                Ok(())
            }
            "pong" | "arkanoid" => {
                for result in self.app.dispatch_batch(event(
                    "store.sources.tick.event.frame",
                    SourceValue::EmptyRecord,
                ))? {
                    self.backend.apply_patches(&result.patches)?;
                }
                self.backend.render_frame_ready()?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn render_gui_frame(
        &mut self,
        width: u32,
        height: u32,
        examples: &[&str],
        current_index: usize,
    ) -> Result<RgbaFrame> {
        self.advance_live_frame()?;
        let controls = native_manual_controls(&self.example).join(" | ");
        Ok(RgbaFrame {
            width,
            height,
            rgba: rasterize_native_gui_frame(
                width,
                height,
                examples,
                current_index,
                self.backend.frame_scene(),
                self.backend.frame_text(),
                &controls,
            ),
        })
    }

    fn visible_todo_ids(&self) -> Vec<String> {
        self.app
            .snapshot()
            .values
            .get("store.visible_todos_ids")
            .and_then(|value| value.as_array())
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_u64().map(|id| id.to_string()))
                    .collect()
            })
            .unwrap_or_else(|| {
                self.app
                    .snapshot()
                    .values
                    .get("store.todos_ids")
                    .and_then(|value| value.as_array())
                    .map(|ids| {
                        ids.iter()
                            .filter_map(|id| id.as_u64().map(|id| id.to_string()))
                            .collect()
                    })
                    .unwrap_or_default()
            })
    }

    fn completed_todo_count(&self) -> i64 {
        self.app
            .snapshot()
            .values
            .get("store.completed_todos_count")
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
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
        surface_frame: Option<AppWindowSurfaceFrameProof>,
        hold: Duration,
    ) -> Result<NativeManualInputProof> {
        let frame = self.backend.capture_frame()?;
        Ok(NativeManualInputProof {
            app_window,
            surface_frame,
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
}

fn run_native_manual_input_session(
    example: &str,
    dir: &Path,
    hold: Duration,
) -> Result<NativeManualInputProof> {
    let state = NativeManualState::new(example)?;
    let example_static = *list_examples()
        .iter()
        .find(|candidate| **candidate == example)
        .with_context(|| format!("unknown native manual example `{example}`"))?;
    let examples = vec![example_static];
    let (app_window, state, surface_frame) = run_rgba_input_session_with_proof(
        format!("Boon {example} native app_window"),
        hold,
        Duration::from_millis(16),
        state,
        |state, sample| state.handle_sample(sample),
        move |state, width, height| state.render_gui_frame(width, height, &examples, 0),
    )?;
    let proof = state.proof(app_window, surface_frame, hold)?;
    fs::write(
        dir.join("manual-controls.txt"),
        native_manual_controls(example).join("\n"),
    )?;
    Ok(proof)
}

fn run_native_visible_surface_probe(
    example: &str,
    dir: &Path,
) -> Result<AppWindowSurfaceFrameProof> {
    let out = dir.join("visible-surface-frame.json");
    let output = Command::new(std::env::current_exe()?)
        .arg("__native-surface-probe")
        .arg("--example")
        .arg(example)
        .arg("--out")
        .arg(&out)
        .stdin(Stdio::null())
        .output()
        .context("spawning native app_window visible surface probe helper")?;
    fs::write(
        dir.join("visible-surface-helper.log"),
        [output.stdout.as_slice(), output.stderr.as_slice()].concat(),
    )?;
    if !output.status.success() {
        bail!(
            "native app_window visible surface probe helper exited with {}",
            output.status
        );
    }
    let surface_frame: AppWindowSurfaceFrameProof = serde_json::from_slice(&fs::read(&out)?)?;
    if !surface_frame.passed {
        bail!(
            "surface frame failed: nonblank={} colors={} size_matches={} configured={}x{} final_surface={}x{}",
            surface_frame.nonblank,
            surface_frame.distinct_sampled_colors,
            surface_frame.size_matches_final_surface,
            surface_frame.configured_width,
            surface_frame.configured_height,
            surface_frame.final_surface_width,
            surface_frame.final_surface_height,
        );
    }
    Ok(surface_frame)
}

fn run_native_close_probe(example: &str, dir: &Path) -> Result<AppWindowCloseProof> {
    let out = dir.join("close-button.json");
    let output = Command::new(std::env::current_exe()?)
        .arg("__native-close-probe")
        .arg("--example")
        .arg(example)
        .arg("--out")
        .arg(&out)
        .stdin(Stdio::null())
        .output()
        .context("spawning native app_window close-button probe helper")?;
    fs::write(
        dir.join("close-button-helper.log"),
        [output.stdout.as_slice(), output.stderr.as_slice()].concat(),
    )?;
    if !output.status.success() {
        bail!(
            "native app_window close-button probe helper exited with {}",
            output.status
        );
    }
    let close: AppWindowCloseProof = serde_json::from_slice(&fs::read(&out)?)?;
    if !close.passed {
        bail!(
            "close-button proof failed: requested={} observed_closed={} presented_before_close={} iterations={}",
            close.requested_close,
            close.observed_closed,
            close.presented_before_close,
            close.iterations_after_close,
        );
    }
    Ok(close)
}

pub fn native_visible_surface_probe_helper(example: &str, out: &Path) -> Result<()> {
    let dir = out
        .parent()
        .context("native visible surface proof output has no parent directory")?;
    let proof = run_native_manual_input_session(example, dir, Duration::ZERO)?;
    let surface_frame = proof
        .surface_frame
        .context("native app_window RGBA session did not produce a visible surface proof")?;
    fs::write(out, serde_json::to_vec_pretty(&surface_frame)?)?;
    if !surface_frame.passed {
        bail!(
            "surface frame failed: nonblank={} colors={} size_matches={} configured={}x{} final_surface={}x{}",
            surface_frame.nonblank,
            surface_frame.distinct_sampled_colors,
            surface_frame.size_matches_final_surface,
            surface_frame.configured_width,
            surface_frame.configured_height,
            surface_frame.final_surface_width,
            surface_frame.final_surface_height,
        );
    }
    Ok(())
}

pub fn native_close_probe_helper(example: &str, out: &Path) -> Result<()> {
    let examples = list_examples().to_vec();
    let mut state = NativeManualState::new_for_playground(example)?;
    let proof = run_close_probe(
        format!("Boon {example} close probe"),
        move |width, height| state.render_gui_frame(width, height, &examples, 0),
    )?;
    fs::write(out, serde_json::to_vec_pretty(&proof)?)?;
    if !proof.passed {
        bail!(
            "close-button proof failed: requested={} observed_closed={} presented_before_close={} iterations={}",
            proof.requested_close,
            proof.observed_closed,
            proof.presented_before_close,
            proof.iterations_after_close,
        );
    }
    Ok(())
}

fn native_manual_controls(example: &str) -> Vec<String> {
    match example {
        "counter" | "counter_hold" => {
            vec!["click the visible Increment button to increment".to_string()]
        }
        "interval" | "interval_hold" => {
            vec!["live clock ticks automatically in playground mode".to_string()]
        }
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
            "frames advance automatically in playground mode".to_string(),
        ],
        _ => Vec::new(),
    }
}

pub fn run_native_playground(initial_example: &str, hold: Duration) -> Result<()> {
    let state = NativePlaygroundState::new(initial_example)?;
    let _ = run_rgba_input_session(
        "Boon native playground",
        hold,
        Duration::from_millis(8),
        state,
        |state, sample| state.handle_sample(sample),
        |state, width, height| state.render_gui_frame(width, height),
    )?;
    Ok(())
}

struct NativePlaygroundState {
    examples: Vec<&'static str>,
    current_index: usize,
    states: Vec<Option<NativeManualState>>,
    switches: usize,
}

impl NativePlaygroundState {
    fn new(initial_example: &str) -> Result<Self> {
        let examples = list_examples().to_vec();
        let current_index = examples
            .iter()
            .position(|example| *example == initial_example)
            .with_context(|| format!("unknown native playground example `{initial_example}`"))?;
        let mut states = Vec::with_capacity(examples.len());
        for example in &examples {
            states.push(Some(NativeManualState::new_for_playground(example)?));
        }
        Ok(Self {
            examples,
            current_index,
            states,
            switches: 0,
        })
    }

    fn handle_sample(&mut self, sample: AppWindowInputSample) -> Result<()> {
        if sample.left_clicked
            && let Some(index) = NativeGuiLayout::from_sample(&sample)
                .sidebar_example_index(&sample, self.examples.len())
        {
            self.switch_to(index)?;
            return Ok(());
        }
        for key in &sample.newly_pressed_keys {
            if self.handle_switch_key(key)? {
                return Ok(());
            }
        }
        self.current_state_mut()?.handle_sample(sample)
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
        self.switches += 1;
        Ok(())
    }

    fn render_gui_frame(&mut self, width: u32, height: u32) -> Result<RgbaFrame> {
        let current_index = self.current_index;
        let examples = self.examples.clone();
        self.current_state_mut()?
            .render_gui_frame(width, height, &examples, current_index)
    }

    fn current_state_mut(&mut self) -> Result<&mut NativeManualState> {
        self.states
            .get_mut(self.current_index)
            .and_then(Option::as_mut)
            .with_context(|| {
                format!(
                    "missing cached native playground state for {}",
                    self.examples[self.current_index]
                )
            })
    }

    fn current_state(&self) -> Result<&NativeManualState> {
        self.states
            .get(self.current_index)
            .and_then(Option::as_ref)
            .with_context(|| {
                format!(
                    "missing cached native playground state for {}",
                    self.examples[self.current_index]
                )
            })
    }

    fn snapshot(&self) -> Result<boon_runtime::AppSnapshot> {
        Ok(self.current_state()?.app.snapshot())
    }

    fn errors_empty(&self) -> Result<bool> {
        Ok(self.current_state()?.errors.is_empty())
    }

    fn visible_todo_ids(&self) -> Result<Vec<String>> {
        Ok(self.current_state()?.visible_todo_ids())
    }
}

struct NativeGuiLayout {
    sidebar_w: f64,
    content_x: f64,
    content_y: f64,
    content_side: f64,
}

impl NativeGuiLayout {
    fn from_sample(sample: &AppWindowInputSample) -> Self {
        let width = sample.mouse_window_width.unwrap_or(1120.0).max(1.0);
        let height = sample.mouse_window_height.unwrap_or(760.0).max(1.0);
        let sidebar_w = 236.0_f64.min(width / 2.0);
        let toolbar_h = 54.0_f64.min(height / 3.0);
        let preview_x = sidebar_w + 24.0;
        let preview_y = toolbar_h + 24.0;
        let preview_w = (width - preview_x - 24.0).max(1.0);
        let preview_h = (height - preview_y - 48.0).max(1.0);
        let content_side = preview_w.min(preview_h).max(1.0);
        let content_x = preview_x + (preview_w - content_side) / 2.0;
        let content_y = preview_y + (preview_h - content_side) / 2.0;
        Self {
            sidebar_w,
            content_x,
            content_y,
            content_side,
        }
    }

    fn preview_virtual(&self, sample: &AppWindowInputSample) -> Option<(f64, f64)> {
        let x = sample.mouse_x?;
        let y = sample.mouse_y?;
        if x < self.content_x
            || y < self.content_y
            || x > self.content_x + self.content_side
            || y > self.content_y + self.content_side
        {
            return None;
        }
        Some((
            (x - self.content_x) * 1000.0 / self.content_side,
            (y - self.content_y) * 1000.0 / self.content_side,
        ))
    }

    fn sidebar_example_index(&self, sample: &AppWindowInputSample, len: usize) -> Option<usize> {
        let x = sample.mouse_x?;
        let y = sample.mouse_y?;
        if x >= self.sidebar_w || y < 78.0 {
            return None;
        }
        let index = ((y - 78.0) / 30.0).floor() as usize;
        (index < len).then_some(index)
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
        "Semicolon" => ':',
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
    visible_surface_frame: Option<&AppWindowSurfaceFrameProof>,
) -> Result<GateResult> {
    let mut app = app(name)?;
    let mut backend = WgpuBackend::headless_real(1280, 720)?;
    let initial = backend.load(&mut app)?;
    let native_script = run_native_scripted_scenario(name, &mut app, &mut backend)?;
    let timing = browser_timing_gate(name, &mut app, &mut backend)?;
    let frame = backend.capture_frame()?;
    let frame_png = write_wgpu_frame_png(&backend, dir, "frame.png")?;
    let visual_contract = native_visual_contract(name, dir, &frame_png)?;
    let playground_interactions = run_native_playground_interaction_scenarios(name, dir)?;
    let replay = replay_native_app_window(name, &app.snapshot(), frame.rgba_hash.as_deref())?;
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
            "visual_contract": &visual_contract,
            "visible_surface_frame": visible_surface_frame,
            "app_window": smoke,
            "wgpu_metadata": backend.metadata(),
            "source_inventory": source_inventory,
            "snapshot": app.snapshot(),
            "frame": frame,
            "scenario_steps": replay_steps(name),
            "human_like_interactions": human_like_interactions(name),
            "native_input_mapping": native_script,
            "native_playground_interactions": playground_interactions,
            "manual_controls": native_manual_controls(name),
            "manual_preview_hold_ms": hold.as_millis(),
        }))?,
    )?;
    let timing_passed = timing
        .get("passed")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let visible_surface_passed = visible_surface_frame.is_none_or(|proof| proof.passed);
    let playground_interactions_passed = playground_interactions
        .as_ref()
        .is_none_or(|proof| proof.passed);
    let passed = frame.rgba_hash.is_some()
        && frame.rgba_hash.as_deref() != Some("")
        && frame_png.nonblank
        && source_count > 0
        && timing_passed
        && visible_surface_passed
        && playground_interactions_passed
        && visual_contract
            .get("passed")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        && native_script.passed
        && replay.passed;
    Ok(GateResult {
        backend: Backend::NativeAppWindow,
        example: name.to_string(),
        passed,
        frame_hash: frame.rgba_hash,
        artifact_dir: dir.to_path_buf(),
        message: if passed {
            "passed native app_window surface creation/present, visible surface readback/size proof, synthetic human-like scenario dispatch, internal framebuffer readback, graphical PNG frame artifact, visual contract, timing evidence, source inventory, and replay gate".to_string()
        } else if !visible_surface_passed {
            "native app_window visible surface readback or live-size proof failed".to_string()
        } else if !playground_interactions_passed {
            "native app_window playground interaction scenarios failed".to_string()
        } else {
            "native app_window example scenario, timing, visual contract, replay, source inventory, or frame hash gate failed".to_string()
        },
    })
}

fn native_visual_contract(
    name: &str,
    dir: &Path,
    frame_png: &FrameImageArtifact,
) -> Result<serde_json::Value> {
    let mut proof = json!({
        "example": name,
        "passed": frame_png.nonblank
            && frame_png.distinct_sampled_colors >= 8
            && frame_png.byte_len > 1024
            && frame_png.rgba_hash.len() == 64,
        "frame": frame_png,
        "checks": [
            "nonblank framebuffer",
            "multiple sampled colors",
            "non-empty PNG artifact",
            "stable RGBA hash"
        ],
    });
    if matches!(name, "todo_mvc" | "todo_mvc_physical") {
        let root = repo_root_from_path(dir)?;
        let visual_path = root.join("examples/todo_mvc/expected.visual.json");
        let visual: serde_json::Value = serde_json::from_slice(&fs::read(&visual_path)?)?;
        let reference = visual
            .get("reference")
            .and_then(|value| value.as_str())
            .context("expected.visual.json missing reference")?;
        let expected_hash = visual
            .get("reference_sha256")
            .and_then(|value| value.as_str())
            .context("expected.visual.json missing reference_sha256")?;
        let reference_path = root.join("examples/todo_mvc").join(reference);
        let actual_hash = hex::encode(Sha256::digest(fs::read(&reference_path)?));
        let reference_ok = actual_hash == expected_hash;
        proof["todo_mvc_reference"] = json!({
            "path": reference_path,
            "expected_sha256": expected_hash,
            "actual_sha256": actual_hash,
            "passed": reference_ok,
            "visual_spec": visual,
        });
        proof["passed"] = json!(
            proof
                .get("passed")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
                && reference_ok
        );
    }
    fs::write(
        dir.join("visual-contract.json"),
        serde_json::to_vec_pretty(&proof)?,
    )?;
    Ok(proof)
}

fn repo_root_from_path(path: &Path) -> Result<PathBuf> {
    let mut dir = path.canonicalize()?;
    loop {
        if dir.join("IMPLEMENTATION_PLAN.md").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!(
                "could not find repo root containing IMPLEMENTATION_PLAN.md from {}",
                path.display()
            );
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct NativeScriptProof {
    passed: bool,
    actions: Vec<String>,
    batches: Vec<serde_json::Value>,
    snapshot_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct NativePlaygroundInteractionProof {
    example: String,
    window_width: u32,
    window_height: u32,
    scenarios: Vec<NativePlaygroundScenarioProof>,
    timing: Option<serde_json::Value>,
    passed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct NativePlaygroundScenarioProof {
    name: String,
    steps: Vec<NativePlaygroundStepProof>,
    final_snapshot: boon_runtime::AppSnapshot,
    final_frame_hash: String,
    assertions: Vec<String>,
    passed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct NativePlaygroundStepProof {
    action: String,
    sample: AppWindowInputSample,
    current_example: String,
    snapshot_hash: String,
    frame_hash: String,
}

fn run_native_playground_interaction_scenarios(
    name: &str,
    dir: &Path,
) -> Result<Option<NativePlaygroundInteractionProof>> {
    let proof = match name {
        "todo_mvc" | "todo_mvc_physical" => run_todomvc_native_playground_scenarios(name)?,
        "counter" | "counter_hold" => run_counter_native_playground_scenarios(name)?,
        "interval" | "interval_hold" => run_interval_native_playground_scenarios(name)?,
        "cells" => run_cells_native_playground_scenarios(name)?,
        "pong" | "arkanoid" => run_game_native_playground_scenarios(name)?,
        _ => return Ok(None),
    };
    fs::write(
        dir.join("playground-interactions.json"),
        serde_json::to_vec_pretty(&proof)?,
    )?;
    if !proof.passed {
        bail!("native playground interaction scenarios failed for {name}");
    }
    Ok(Some(proof))
}

fn run_todomvc_native_playground_scenarios(name: &str) -> Result<NativePlaygroundInteractionProof> {
    let scenarios = vec![
        todomvc_playground_add_toggle_filter_clear(name)?,
        todomvc_playground_edit_remove(name)?,
        todomvc_playground_reject_empty_and_outside_click(name)?,
    ];
    let timing = todomvc_native_playground_mouse_timing(name)?;
    let timing_passed = timing
        .get("passed")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let passed = scenarios.iter().all(|scenario| scenario.passed) && timing_passed;
    Ok(NativePlaygroundInteractionProof {
        example: name.to_string(),
        window_width: 1020,
        window_height: 1082,
        scenarios,
        timing: Some(timing),
        passed,
    })
}

fn todomvc_native_playground_mouse_timing(name: &str) -> Result<serde_json::Value> {
    let cases = vec![
        measure_todomvc_native_mouse_rerender(
            name,
            "native_todomvc_mouse_check_one_100_under_16ms",
            16.0,
            |_, _| preview_click_sample(1020.0, 1082.0, 236.0, 262.0),
        )?,
        measure_todomvc_native_mouse_rerender(
            name,
            "native_todomvc_mouse_toggle_all_100_under_25ms",
            25.0,
            |_, _| preview_click_sample(1020.0, 1082.0, 236.0, 196.0),
        )?,
    ];
    Ok(timing_cases(cases))
}

fn measure_todomvc_native_mouse_rerender(
    name: &str,
    scenario: &str,
    max_budget_ms: f64,
    mut sample: impl FnMut(&mut NativePlaygroundState, usize) -> AppWindowInputSample,
) -> Result<serde_json::Value> {
    let mut state = todomvc_playground_state(name)?;
    ensure_todo_count_native_playground(&mut state, 100)?;
    for i in 0..5 {
        let click = sample(&mut state, i);
        state.handle_sample(click)?;
        let _ = state.render_gui_frame(1020, 1082)?;
    }
    let mut samples = Vec::new();
    for i in 0..30 {
        let click = sample(&mut state, i + 5);
        let start = Instant::now();
        state.handle_sample(click)?;
        let _ = state.render_gui_frame(1020, 1082)?;
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    timing_sample(
        scenario,
        max_budget_ms,
        max_budget_ms,
        Some(max_budget_ms),
        samples,
    )
}

fn ensure_todo_count_native_playground(
    playground: &mut NativePlaygroundState,
    target: i64,
) -> Result<()> {
    loop {
        let current = playground
            .snapshot()?
            .values
            .get("store.todos_count")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if current >= target {
            return Ok(());
        }
        let title = format!("Todo {next:03}", next = current + 1);
        dispatch_native_manual(
            playground.current_state_mut()?,
            state(
                "store.sources.new_todo_input.text",
                SourceValue::Text(title),
            ),
        )?;
        dispatch_native_manual(
            playground.current_state_mut()?,
            event(
                "store.sources.new_todo_input.event.key_down.key",
                SourceValue::Tag("Enter".to_string()),
            ),
        )?;
    }
}

fn dispatch_native_manual(manual: &mut NativeManualState, batch: SourceBatch) -> Result<()> {
    for result in manual.app.dispatch_batch(batch)? {
        manual.backend.apply_patches(&result.patches)?;
    }
    manual.backend.render_frame_ready()?;
    Ok(())
}

fn run_counter_native_playground_scenarios(name: &str) -> Result<NativePlaygroundInteractionProof> {
    let mut state = playground_state_for(name)?;
    let mut steps = Vec::new();
    let initial = state
        .snapshot()?
        .values
        .get("counter")
        .cloned()
        .unwrap_or(json!(0));
    play_click(
        &mut state,
        &mut steps,
        "click counter preview background outside button",
        40.0,
        40.0,
    )?;
    expect(
        state.snapshot()?.values.get("counter"),
        initial,
        "counter unchanged after outside click",
    )?;
    play_click(
        &mut state,
        &mut steps,
        "click visible increment button",
        500.0,
        434.0,
    )?;
    expect(
        state.snapshot()?.values.get("counter"),
        json!(1),
        "counter incremented only from button",
    )?;
    let scenario = finish_playground_scenario(
        "counter_button_only",
        state,
        steps,
        vec![
            "sidebar selection used".to_string(),
            "outside click did not increment".to_string(),
            "button click incremented".to_string(),
        ],
    )?;
    Ok(single_playground_proof(name, scenario))
}

fn run_interval_native_playground_scenarios(
    name: &str,
) -> Result<NativePlaygroundInteractionProof> {
    let mut state = playground_state_for(name)?;
    let mut steps = Vec::new();
    let first = state.render_gui_frame(1020, 1082)?;
    let first_hash = hash_rgba(first.width, first.height, &first.rgba);
    state.current_state_mut()?.last_auto_tick = Instant::now() - Duration::from_millis(1_200);
    record_playground_step(
        &mut state,
        &mut steps,
        "wait live interval tick and render",
        AppWindowInputSample {
            mouse_window_width: Some(1020.0),
            mouse_window_height: Some(1082.0),
            ..AppWindowInputSample::default()
        },
    )?;
    let interval_count = state
        .snapshot()?
        .values
        .get("interval_count")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    if interval_count == 0 {
        bail!("interval playground did not tick from live host time");
    }
    if steps
        .last()
        .is_some_and(|step| step.frame_hash == first_hash)
    {
        bail!("interval playground frame hash did not change after live tick");
    }
    let scenario = finish_playground_scenario(
        "interval_live_tick",
        state,
        steps,
        vec![
            "sidebar selection used".to_string(),
            "live tick advanced state".to_string(),
            "frame hash changed".to_string(),
        ],
    )?;
    Ok(single_playground_proof(name, scenario))
}

fn run_cells_native_playground_scenarios(name: &str) -> Result<NativePlaygroundInteractionProof> {
    let mut state = playground_state_for(name)?;
    let mut steps = Vec::new();
    play_click(&mut state, &mut steps, "click A1 grid cell", 146.0, 219.0)?;
    play_text(
        &mut state,
        &mut steps,
        "type A1 cell value character-by-character",
        "1",
    )?;
    play_key(
        &mut state,
        &mut steps,
        "press Enter in A1 cell editor",
        "Return",
    )?;
    expect(
        state.snapshot()?.values.get("cells.A1"),
        json!("1"),
        "cells A1 after native playground edit",
    )?;
    play_click(&mut state, &mut steps, "click A2 grid cell", 146.0, 257.0)?;
    play_text(
        &mut state,
        &mut steps,
        "type A2 cell value character-by-character",
        "2",
    )?;
    play_key(
        &mut state,
        &mut steps,
        "press Enter in A2 cell editor",
        "Return",
    )?;
    play_click(&mut state, &mut steps, "click B1 grid cell", 238.0, 219.0)?;
    play_text(
        &mut state,
        &mut steps,
        "type B1 add formula character-by-character",
        "=add(a1, a2)",
    )?;
    play_key(
        &mut state,
        &mut steps,
        "press Enter in B1 formula editor",
        "Return",
    )?;
    expect(
        state.snapshot()?.values.get("cells.B1"),
        json!("3"),
        "cells B1 after native playground add formula",
    )?;
    expect(
        state.snapshot()?.values.get("cells.selected_formula"),
        json!("=add(a1, a2)"),
        "cells formula bar shows selected B1 formula",
    )?;
    play_click(&mut state, &mut steps, "click B2 grid cell", 238.0, 257.0)?;
    play_text(
        &mut state,
        &mut steps,
        "type B2 sum formula character-by-character",
        "=sum(a1:a2)",
    )?;
    play_key(
        &mut state,
        &mut steps,
        "press Enter in B2 formula editor",
        "Return",
    )?;
    expect(
        state.snapshot()?.values.get("cells.B2"),
        json!("3"),
        "cells B2 after native playground sum formula",
    )?;
    play_click(
        &mut state,
        &mut steps,
        "click A2 grid cell for update",
        146.0,
        257.0,
    )?;
    play_key(
        &mut state,
        &mut steps,
        "clear old A2 value with Backspace",
        "Backspace",
    )?;
    play_text(
        &mut state,
        &mut steps,
        "type updated A2 value character-by-character",
        "5",
    )?;
    play_key(
        &mut state,
        &mut steps,
        "press Enter after updating A2",
        "Return",
    )?;
    expect(
        state.snapshot()?.values.get("cells.B1"),
        json!("6"),
        "cells B1 recomputes after A2 update",
    )?;
    expect(
        state.snapshot()?.values.get("cells.B2"),
        json!("6"),
        "cells B2 recomputes after A2 update",
    )?;
    let scenario = finish_playground_scenario(
        "cells_click_type_enter",
        state,
        steps,
        vec![
            "sidebar selection used".to_string(),
            "grid cells clicked".to_string(),
            "cell text and formulas typed character-by-character".to_string(),
            "formula bar exposes selected formula".to_string(),
            "dependent formulas recompute after source cell update".to_string(),
        ],
    )?;
    Ok(single_playground_proof(name, scenario))
}

fn run_game_native_playground_scenarios(name: &str) -> Result<NativePlaygroundInteractionProof> {
    let mut state = playground_state_for(name)?;
    let mut steps = Vec::new();
    let first = state.render_gui_frame(1020, 1082)?;
    let first_hash = hash_rgba(first.width, first.height, &first.rgba);
    let initial_snapshot = state.snapshot()?;
    let initial_x = snapshot_i64(&initial_snapshot, "game.ball_x")?;
    let initial_y = snapshot_i64(&initial_snapshot, "game.ball_y")?;
    let initial_dx = snapshot_i64(&initial_snapshot, "game.ball_dx")?;
    let initial_dy = snapshot_i64(&initial_snapshot, "game.ball_dy")?;
    let initial_bricks = snapshot_i64(&initial_snapshot, "game.obstacles_live_count").unwrap_or(0);
    let (axis, first_key, second_key, first_label, second_label) = if name == "arkanoid" {
        (
            "game.control_x",
            "LeftArrow",
            "RightArrow",
            "press ArrowLeft horizontal paddle control",
            "press ArrowRight horizontal paddle control and advance frame",
        )
    } else {
        (
            "game.control_y",
            "UpArrow",
            "DownArrow",
            "press ArrowUp vertical paddle control",
            "press ArrowDown vertical paddle control and advance frame",
        )
    };
    let initial_paddle = snapshot_i64(&initial_snapshot, axis)?;
    play_key(&mut state, &mut steps, first_label, first_key)?;
    let after_first = snapshot_i64(&state.snapshot()?, axis)?;
    if after_first >= initial_paddle {
        bail!(
            "{name} playground {first_key} did not move paddle: {initial_paddle} -> {after_first}"
        );
    }
    state.current_state_mut()?.last_auto_tick = Instant::now() - Duration::from_millis(60);
    play_key(&mut state, &mut steps, second_label, second_key)?;
    let after_second = snapshot_i64(&state.snapshot()?, axis)?;
    if after_second <= after_first {
        bail!(
            "{name} playground {second_key} did not move paddle: {after_first} -> {after_second}"
        );
    }
    record_playground_step(
        &mut state,
        &mut steps,
        "hold game control key and receive key-repeat sample",
        repeated_key_sample(second_key),
    )?;
    let after_repeat = snapshot_i64(&state.snapshot()?, axis)?;
    if after_repeat <= after_second {
        bail!(
            "{name} playground held {second_key} did not continue moving paddle: {after_second} -> {after_repeat}"
        );
    }
    let mut saw_ball_move = false;
    let mut saw_collision = false;
    let mut saw_brick_hit = name != "arkanoid";
    for tick_idx in 0..48 {
        state.current_state_mut()?.last_auto_tick = Instant::now() - Duration::from_millis(60);
        let frame = state.render_gui_frame(1020, 1082)?;
        steps.push(NativePlaygroundStepProof {
            action: format!("advance deterministic game physics frame {tick_idx}"),
            sample: AppWindowInputSample::default(),
            current_example: state.examples[state.current_index].to_string(),
            snapshot_hash: snapshot_hash(&state.snapshot()?)?,
            frame_hash: hash_rgba(frame.width, frame.height, &frame.rgba),
        });
        let snapshot = state.snapshot()?;
        let x = snapshot_i64(&snapshot, "game.ball_x")?;
        let y = snapshot_i64(&snapshot, "game.ball_y")?;
        let dx = snapshot_i64(&snapshot, "game.ball_dx")?;
        let dy = snapshot_i64(&snapshot, "game.ball_dy")?;
        saw_ball_move |= x != initial_x || y != initial_y;
        saw_collision |= dx.signum() != initial_dx.signum() || dy.signum() != initial_dy.signum();
        if name == "arkanoid" {
            saw_brick_hit |= snapshot_i64(&snapshot, "game.obstacles_live_count")? < initial_bricks;
        }
    }
    let frame = state
        .snapshot()?
        .values
        .get("game.frame")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    if frame == 0 {
        bail!("{name} playground did not advance autonomous game frame");
    }
    if !saw_ball_move {
        bail!("{name} playground ball did not move during physics ticks");
    }
    if !saw_collision {
        bail!("{name} playground did not observe a ball collision reversing velocity");
    }
    if !saw_brick_hit {
        bail!("{name} playground did not remove a brick during Arkanoid collision scenario");
    }
    if steps
        .last()
        .is_some_and(|step| step.frame_hash == first_hash)
    {
        bail!("{name} playground frame hash did not change after autonomous frame");
    }
    let scenario = finish_playground_scenario(
        "game_keyboard_and_auto_frame",
        state,
        steps,
        vec![
            "sidebar selection used".to_string(),
            "keyboard controls moved the paddle".to_string(),
            "held keyboard repeat continued moving the paddle".to_string(),
            "autonomous frame advanced".to_string(),
            "ball position changed from runtime physics".to_string(),
            "ball collision reversed velocity".to_string(),
            "arkanoid brick collision removes bricks when applicable".to_string(),
        ],
    )?;
    Ok(single_playground_proof(name, scenario))
}

fn snapshot_i64(snapshot: &boon_runtime::AppSnapshot, key: &str) -> Result<i64> {
    snapshot
        .values
        .get(key)
        .and_then(|value| value.as_i64())
        .with_context(|| format!("missing numeric snapshot key `{key}`"))
}

fn single_playground_proof(
    name: &str,
    scenario: NativePlaygroundScenarioProof,
) -> NativePlaygroundInteractionProof {
    let passed = scenario.passed;
    NativePlaygroundInteractionProof {
        example: name.to_string(),
        window_width: 1020,
        window_height: 1082,
        scenarios: vec![scenario],
        timing: None,
        passed,
    }
}

fn playground_state_for(name: &str) -> Result<NativePlaygroundState> {
    let mut state = NativePlaygroundState::new("counter")?;
    let index = state
        .examples
        .iter()
        .position(|example| *example == name)
        .with_context(|| format!("missing native playground example `{name}`"))?;
    let y = 78.0 + index as f64 * 30.0 + 15.0;
    state.handle_sample(click_sample(40.0, y))?;
    if state.examples[state.current_index] != name {
        bail!(
            "native playground sidebar switched to {}, expected {name}",
            state.examples[state.current_index]
        );
    }
    Ok(state)
}

fn todomvc_playground_state(name: &str) -> Result<NativePlaygroundState> {
    playground_state_for(name)
}

fn todomvc_playground_add_toggle_filter_clear(name: &str) -> Result<NativePlaygroundScenarioProof> {
    let mut state = todomvc_playground_state(name)?;
    let mut steps = Vec::new();
    let sidebar_y = 78.0 + state.current_index as f64 * 30.0 + 15.0;
    record_playground_step(
        &mut state,
        &mut steps,
        "sidebar click selects TodoMVC",
        click_sample(40.0, sidebar_y),
    )?;
    play_click(&mut state, &mut steps, "click new todo input", 320.0, 196.0)?;
    play_text(
        &mut state,
        &mut steps,
        "type todo text character-by-character",
        "read docs",
    )?;
    play_key(&mut state, &mut steps, "press Enter to add todo", "Return")?;
    expect(
        state.snapshot()?.values.get("store.todos_count"),
        json!(3),
        "TodoMVC count after playground add",
    )?;
    play_click(
        &mut state,
        &mut steps,
        "click first todo checkbox",
        236.0,
        262.0,
    )?;
    expect(
        state.snapshot()?.values.get("store.completed_todos_count"),
        json!(1),
        "TodoMVC completed count after playground checkbox",
    )?;
    if name == "todo_mvc" {
        play_todo_footer_click(&mut state, &mut steps, "click completed filter", 592.0)?;
        expect(
            state.snapshot()?.values.get("store.selected_filter"),
            json!("completed"),
            "TodoMVC completed filter selected",
        )?;
        play_todo_footer_click(&mut state, &mut steps, "click active filter", 482.0)?;
        expect(
            state.snapshot()?.values.get("store.selected_filter"),
            json!("active"),
            "TodoMVC active filter selected",
        )?;
        play_todo_footer_click(&mut state, &mut steps, "click all filter", 386.0)?;
        expect(
            state.snapshot()?.values.get("store.selected_filter"),
            json!("all"),
            "TodoMVC all filter selected",
        )?;
    }
    play_todo_footer_click(&mut state, &mut steps, "click clear completed", 720.0)?;
    expect(
        state.snapshot()?.values.get("store.todos_count"),
        json!(2),
        "TodoMVC count after playground clear completed",
    )?;
    finish_playground_scenario(
        "todo_mvc_add_toggle_filter_clear",
        state,
        steps,
        vec![
            "sidebar selection used".to_string(),
            "typed text character-by-character".to_string(),
            "checkbox/filter/clear regions clicked".to_string(),
        ],
    )
}

fn todomvc_playground_edit_remove(name: &str) -> Result<NativePlaygroundScenarioProof> {
    let mut state = todomvc_playground_state(name)?;
    let mut steps = Vec::new();
    play_click(
        &mut state,
        &mut steps,
        "click first todo row text",
        320.0,
        262.0,
    )?;
    play_text(
        &mut state,
        &mut steps,
        "append edit text character-by-character",
        " updated",
    )?;
    play_key(
        &mut state,
        &mut steps,
        "press Enter to commit edit",
        "Return",
    )?;
    let visible_ids = state.visible_todo_ids()?;
    let first_id = visible_ids
        .first()
        .context("TodoMVC edit scenario has no visible first todo")?;
    let title_key = format!("store.todos[{first_id}].title");
    let edited_title = state
        .snapshot()?
        .values
        .get(&title_key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    if !edited_title.ends_with(" updated") {
        bail!("TodoMVC edit did not update title: {edited_title}");
    }
    play_click(
        &mut state,
        &mut steps,
        "click second todo remove button",
        760.0,
        324.0,
    )?;
    expect(
        state.snapshot()?.values.get("store.todos_count"),
        json!(1),
        "TodoMVC count after playground remove",
    )?;
    finish_playground_scenario(
        "todo_mvc_edit_remove",
        state,
        steps,
        vec![
            "row text click focused edit input".to_string(),
            "edit text typed character-by-character".to_string(),
            "remove region clicked".to_string(),
        ],
    )
}

fn todomvc_playground_reject_empty_and_outside_click(
    name: &str,
) -> Result<NativePlaygroundScenarioProof> {
    let mut state = todomvc_playground_state(name)?;
    let mut steps = Vec::new();
    let initial_count = state
        .snapshot()?
        .values
        .get("store.todos_count")
        .cloned()
        .unwrap_or(json!(0));
    play_click(
        &mut state,
        &mut steps,
        "click non-control preview background",
        660.0,
        42.0,
    )?;
    expect(
        state.snapshot()?.values.get("store.todos_count"),
        initial_count.clone(),
        "TodoMVC count unchanged after outside click",
    )?;
    play_todo_footer_click(
        &mut state,
        &mut steps,
        "click hidden clear-completed area with no completed todos",
        720.0,
    )?;
    expect(
        state.snapshot()?.values.get("store.todos_count"),
        initial_count.clone(),
        "TodoMVC count unchanged when clear completed is hidden",
    )?;
    play_click(
        &mut state,
        &mut steps,
        "click new todo input for whitespace",
        320.0,
        196.0,
    )?;
    play_text(&mut state, &mut steps, "type whitespace-only text", "   ")?;
    for idx in 0..3 {
        record_playground_step(
            &mut state,
            &mut steps,
            &format!(
                "hold Backspace repeat deletes whitespace character {}",
                idx + 1
            ),
            repeated_key_sample("Delete"),
        )?;
    }
    expect(
        state
            .snapshot()?
            .values
            .get("store.sources.new_todo_input.text"),
        json!(""),
        "TodoMVC repeated Backspace cleared input",
    )?;
    play_key(
        &mut state,
        &mut steps,
        "press Enter to reject whitespace todo",
        "Return",
    )?;
    expect(
        state.snapshot()?.values.get("store.todos_count"),
        initial_count,
        "TodoMVC count unchanged after whitespace input",
    )?;
    finish_playground_scenario(
        "todo_mvc_reject_empty_and_outside_click",
        state,
        steps,
        vec![
            "outside click did not mutate state".to_string(),
            "whitespace-only todo rejected".to_string(),
            "held Backspace repeat deletes text continuously".to_string(),
        ],
    )
}

fn finish_playground_scenario(
    name: &str,
    mut state: NativePlaygroundState,
    steps: Vec<NativePlaygroundStepProof>,
    assertions: Vec<String>,
) -> Result<NativePlaygroundScenarioProof> {
    let frame = state.render_gui_frame(1020, 1082)?;
    let final_frame_hash = hash_rgba(frame.width, frame.height, &frame.rgba);
    Ok(NativePlaygroundScenarioProof {
        name: name.to_string(),
        steps,
        final_snapshot: state.snapshot()?,
        final_frame_hash,
        assertions,
        passed: state.errors_empty()?,
    })
}

fn play_click(
    state: &mut NativePlaygroundState,
    steps: &mut Vec<NativePlaygroundStepProof>,
    action: &str,
    virtual_x: f64,
    virtual_y: f64,
) -> Result<()> {
    record_playground_step(
        state,
        steps,
        action,
        preview_click_sample(1020.0, 1082.0, virtual_x, virtual_y),
    )
}

fn play_todo_footer_click(
    state: &mut NativePlaygroundState,
    steps: &mut Vec<NativePlaygroundStepProof>,
    action: &str,
    virtual_x: f64,
) -> Result<()> {
    let visible_count = state.visible_todo_ids()?.len();
    let footer_y = 160.0 + 72.0 + visible_count as f64 * 62.0 + 27.0;
    play_click(state, steps, action, virtual_x, footer_y)
}

fn play_text(
    state: &mut NativePlaygroundState,
    steps: &mut Vec<NativePlaygroundStepProof>,
    action: &str,
    text: &str,
) -> Result<()> {
    for (index, ch) in text.chars().enumerate() {
        play_key(
            state,
            steps,
            &format!("{action} #{index} `{ch}`"),
            key_name(ch),
        )?;
    }
    Ok(())
}

fn play_key(
    state: &mut NativePlaygroundState,
    steps: &mut Vec<NativePlaygroundStepProof>,
    action: &str,
    key: &str,
) -> Result<()> {
    let sample = key_sample(key);
    record_playground_step(state, steps, action, sample)
}

fn record_playground_step(
    state: &mut NativePlaygroundState,
    steps: &mut Vec<NativePlaygroundStepProof>,
    action: &str,
    sample: AppWindowInputSample,
) -> Result<()> {
    state.handle_sample(sample.clone())?;
    let frame = state.render_gui_frame(1020, 1082)?;
    steps.push(NativePlaygroundStepProof {
        action: action.to_string(),
        sample,
        current_example: state.examples[state.current_index].to_string(),
        snapshot_hash: snapshot_hash(&state.snapshot()?)?,
        frame_hash: hash_rgba(frame.width, frame.height, &frame.rgba),
    });
    Ok(())
}

fn preview_click_sample(
    window_width: f64,
    window_height: f64,
    virtual_x: f64,
    virtual_y: f64,
) -> AppWindowInputSample {
    let base = AppWindowInputSample {
        mouse_window_width: Some(window_width),
        mouse_window_height: Some(window_height),
        ..AppWindowInputSample::default()
    };
    let layout = NativeGuiLayout::from_sample(&base);
    click_sample_at(
        layout.content_x + virtual_x * layout.content_side / 1000.0,
        layout.content_y + virtual_y * layout.content_side / 1000.0,
        window_width,
        window_height,
    )
}

fn click_sample(x: f64, y: f64) -> AppWindowInputSample {
    click_sample_at(x, y, 1020.0, 1082.0)
}

fn click_sample_at(x: f64, y: f64, window_width: f64, window_height: f64) -> AppWindowInputSample {
    AppWindowInputSample {
        mouse_x: Some(x),
        mouse_y: Some(y),
        mouse_window_width: Some(window_width),
        mouse_window_height: Some(window_height),
        left_pressed: true,
        left_clicked: true,
        ..AppWindowInputSample::default()
    }
}

fn key_sample(key: &str) -> AppWindowInputSample {
    AppWindowInputSample {
        mouse_window_width: Some(1020.0),
        mouse_window_height: Some(1082.0),
        pressed_keys: vec![key.to_string()],
        newly_pressed_keys: vec![key.to_string()],
        ..AppWindowInputSample::default()
    }
}

fn repeated_key_sample(key: &str) -> AppWindowInputSample {
    AppWindowInputSample {
        mouse_window_width: Some(1020.0),
        mouse_window_height: Some(1082.0),
        pressed_keys: vec![key.to_string()],
        repeated_keys: vec![key.to_string()],
        ..AppWindowInputSample::default()
    }
}

fn key_name(ch: char) -> &'static str {
    match ch {
        'a' | 'A' => "A",
        'b' | 'B' => "B",
        'c' | 'C' => "C",
        'd' | 'D' => "D",
        'e' | 'E' => "E",
        'f' | 'F' => "F",
        'g' | 'G' => "G",
        'h' | 'H' => "H",
        'i' | 'I' => "I",
        'j' | 'J' => "J",
        'k' | 'K' => "K",
        'l' | 'L' => "L",
        'm' | 'M' => "M",
        'n' | 'N' => "N",
        'o' | 'O' => "O",
        'p' | 'P' => "P",
        'q' | 'Q' => "Q",
        'r' | 'R' => "R",
        's' | 'S' => "S",
        't' | 'T' => "T",
        'u' | 'U' => "U",
        'v' | 'V' => "V",
        'w' | 'W' => "W",
        'x' | 'X' => "X",
        'y' | 'Y' => "Y",
        'z' | 'Z' => "Z",
        '0' => "Num0",
        '1' => "Num1",
        '2' => "Num2",
        '3' => "Num3",
        '4' => "Num4",
        '5' => "Num5",
        '6' => "Num6",
        '7' => "Num7",
        '8' => "Num8",
        '9' => "Num9",
        ' ' => "Space",
        '-' => "Minus",
        '=' => "Equal",
        ',' => "Comma",
        ':' => "Semicolon",
        '.' => "Period",
        '/' => "Slash",
        '(' => "LeftBracket",
        ')' => "RightBracket",
        _ => "Space",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_playground_hit_targets_cover_todomvc_controls() {
        let proof = run_todomvc_native_playground_scenarios("todo_mvc")
            .expect("TodoMVC native playground scenarios should run without a visible window");
        assert!(proof.passed, "{proof:#?}");
        let scenario_names = proof
            .scenarios
            .iter()
            .map(|scenario| scenario.name.as_str())
            .collect::<Vec<_>>();
        assert!(scenario_names.contains(&"todo_mvc_add_toggle_filter_clear"));
        assert!(scenario_names.contains(&"todo_mvc_edit_remove"));
        assert!(scenario_names.contains(&"todo_mvc_reject_empty_and_outside_click"));
    }

    #[test]
    fn native_playground_hit_targets_cover_counter_and_cells() {
        let counter = run_counter_native_playground_scenarios("counter")
            .expect("counter native playground scenario should run without a visible window");
        assert!(counter.passed, "{counter:#?}");

        let cells = run_cells_native_playground_scenarios("cells")
            .expect("cells native playground scenario should run without a visible window");
        assert!(cells.passed, "{cells:#?}");
    }
}
