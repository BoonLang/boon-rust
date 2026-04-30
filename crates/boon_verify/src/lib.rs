use anyhow::{Result, bail};
use boon_backend_app_window::smoke_test as app_window_smoke_test;
use boon_backend_browser::BrowserScenarioInput;
use boon_backend_ratatui::RatatuiBackend;
use boon_backend_wgpu::WgpuBackend;
use boon_examples::{app, list_examples};
use boon_runtime::{BoonApp, SourceBatch, SourceEmission, SourceValue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ReplayProof {
    passed: bool,
    snapshot_hash: String,
    replay_snapshot_hash: String,
    frame_hash: Option<String>,
    replay_frame_hash: Option<String>,
    steps: Vec<String>,
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
        if let Some(capture) = &pty_capture {
            fs::write(dir.join("pty-capture.txt"), capture)?;
        }
        fs::write(
            dir.join("timings.json"),
            serde_json::to_vec_pretty(&timing)?,
        )?;
        fs::write(
            dir.join("trace.json"),
            serde_json::to_vec_pretty(&json!({
                "example": name,
                "mode": if pty { "pty" } else { "buffer" },
                "initial_hash": info.hash,
                "final_hash": stable_sha(&frame.text),
                "pty_capture_hash": pty_capture.as_ref().map(|capture| stable_sha(capture)),
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
            passed: pty_matches && timing_passed && replay.passed,
            frame_hash: Some(stable_sha(&frame.text)),
            artifact_dir: dir,
            message: if pty_matches && timing_passed && replay.passed {
                "passed deterministic semantic/frame text checks and replay gate".to_string()
            } else if !timing_passed {
                "timing budget gate failed".to_string()
            } else if !replay.passed {
                "replay gate failed".to_string()
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

fn replay_steps(name: &str) -> Vec<String> {
    match name {
        "counter" | "counter_hold" => vec![
            "mount".to_string(),
            "10 x store.sources.increment_button.event.press".to_string(),
        ],
        "interval" | "interval_hold" => {
            vec!["mount".to_string(), "advance_fake_time 3000ms".to_string()]
        }
        "todo_mvc" | "todo_mvc_physical" => vec![
            "mount".to_string(),
            "set new_todo_input.text".to_string(),
            "press Enter".to_string(),
            "toggle all".to_string(),
            "toggle item 1".to_string(),
            "timing scenarios".to_string(),
        ],
        "cells" => vec![
            "mount".to_string(),
            "edit A1".to_string(),
            "press Enter".to_string(),
            "timing scenarios".to_string(),
        ],
        "pong" | "arkanoid" => vec!["mount".to_string(), "frame tick".to_string()],
        _ => vec!["unknown".to_string()],
    }
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
                fs::write(
                    dir.join("timings.json"),
                    serde_json::to_vec_pretty(&timing)?,
                )?;
                fs::write(
                    dir.join("trace.json"),
                    serde_json::to_vec_pretty(&json!({
                        "example": name,
                        "mode": "native-wgpu-headless",
                        "initial_hash": info.hash,
                        "final_rgba_hash": frame.rgba_hash,
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
                    passed: timing_passed && replay.passed,
                    frame_hash: frame.rgba_hash,
                    artifact_dir: dir,
                    message: if timing_passed && replay.passed {
                        format!(
                        "passed native wgpu adapter/device, offscreen render, framebuffer readback, deterministic scenario checks, and replay gate ({:?})",
                        backend.metadata()
                    )
                    } else if !replay.passed {
                        "native wgpu replay gate failed".to_string()
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
                let initial = backend.load(&mut app)?;
                run_core_scenario_wgpu(name, &mut app, &mut backend)?;
                let timing = browser_timing_gate(name, &mut app, &mut backend)?;
                let frame = backend.capture_frame()?;
                pending.push((
                    (*name).to_string(),
                    initial.hash,
                    frame.rgba_hash.clone(),
                    serde_json::to_value(backend.metadata())?,
                    serde_json::to_value(app.source_inventory())?,
                    serde_json::to_value(app.snapshot())?,
                    timing,
                ));
            }

            let scenario_inputs = pending
                .iter()
                .map(
                    |(name, _, frame_hash, metadata, source_inventory, snapshot, timing)| {
                        BrowserScenarioInput {
                            example: name.clone(),
                            snapshot: snapshot.clone(),
                            source_inventory: source_inventory.clone(),
                            frame_hash: frame_hash.clone(),
                            timing: timing.clone(),
                            wgpu_metadata: metadata.clone(),
                        }
                    },
                )
                .collect::<Vec<_>>();
            let browser_proofs =
                boon_backend_browser::run_firefox_webgpu_scenarios(&scenario_inputs)?;
            fs::write(
                root_dir.join("scenario-proofs.json"),
                serde_json::to_vec_pretty(&browser_proofs)?,
            )?;

            let mut results = Vec::new();
            for (name, initial_hash, frame_hash, metadata, source_inventory, snapshot, timing) in
                pending
            {
                let dir = artifacts.join(&name).join("browser-firefox-extension");
                fs::create_dir_all(&dir)?;
                let proof = browser_proofs
                    .iter()
                    .find(|proof| proof.example == name)
                    .cloned();
                fs::write(
                    dir.join("trace.json"),
                    serde_json::to_vec_pretty(&json!({
                        "example": name,
                        "mode": "browser-firefox-webgpu-extension",
                        "firefox": capability,
                        "initial_hash": initial_hash,
                        "final_rgba_hash": frame_hash,
                        "metadata": metadata,
                        "source_inventory": source_inventory,
                        "snapshot": snapshot,
                        "browser_proof": proof,
                        "scenario": "firefox-webgpu-webextension-test-api",
                    }))?,
                )?;
                fs::write(
                    dir.join("timings.json"),
                    serde_json::to_vec_pretty(&timing)?,
                )?;
                if let Some(proof) = &proof {
                    fs::write(
                        dir.join("browser-proof.json"),
                        serde_json::to_vec_pretty(proof)?,
                    )?;
                    fs::write(
                        dir.join("replay.json"),
                        serde_json::to_vec_pretty(&json!({
                            "passed": proof.wasm_runner_ok
                                && proof.wasm_frame_hash == frame_hash
                                && proof.wasm_source_count == proof.source_count
                                && proof.errors.is_empty(),
                            "kind": "firefox-wasm-proof-replay",
                            "frame_hash": frame_hash,
                            "wasm_frame_hash": proof.wasm_frame_hash,
                            "source_count": proof.source_count,
                            "wasm_source_count": proof.wasm_source_count,
                            "wasm_snapshot_matches": proof.wasm_snapshot_matches,
                            "wasm_source_inventory_matches": proof.wasm_source_inventory_matches,
                        }))?,
                    )?;
                }
                let passed = frame_hash.is_some()
                    && frame_hash.as_deref() != Some("")
                    && timing
                        .get("passed")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(true)
                    && proof.as_ref().is_some_and(|proof| {
                        proof.navigator_gpu
                            && proof.extension_loaded
                            && proof.native_messaging_connected
                            && proof.test_api_available
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
                            && proof.frame_hash == frame_hash
                            && proof.wasm_frame_hash == frame_hash
                            && proof.timing_passed
                            && proof.errors.is_empty()
                    });
                results.push(GateResult {
                    backend: Backend::BrowserFirefoxWgpu,
                    example: name,
                    passed,
                    frame_hash,
                    artifact_dir: dir,
                    message: if passed {
                        "passed real Firefox WebGPU/WebExtension/native-messaging/Rust-wasm test API proof plus deterministic state, source inventory, frame hash, and timing gate".to_string()
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
                    let owner_id = ((i % 100) + 1).to_string();
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
        _ => Ok(json!({
            "scenario": "not-budgeted",
            "passed": true,
            "warmup_iterations": 0,
            "measured_iterations": 0,
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
                    let owner_id = ((i % 100) + 1).to_string();
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
        _ => Ok(json!({
            "scenario": "not-budgeted",
            "passed": true,
            "warmup_iterations": 0,
            "measured_iterations": 0,
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
    let dir = artifacts.join("native-app-window");
    fs::create_dir_all(&dir)?;
    match app_window_smoke_test() {
        Ok(smoke) => {
            let mut app = app("counter")?;
            let mut backend = WgpuBackend::headless_real(1280, 720)?;
            backend.load(&mut app)?;
            backend.dispatch(
                &mut app,
                event(
                    "store.sources.increment_button.event.press",
                    SourceValue::EmptyRecord,
                ),
            )?;
            let frame = backend.capture_frame()?;
            let passed = frame.rgba_hash.is_some()
                && frame.rgba_hash.as_deref() != Some("")
                && app.snapshot().values.get("counter") == Some(&json!(1));
            fs::write(
                dir.join("trace.json"),
                serde_json::to_vec_pretty(&json!({
                    "app_window": smoke,
                    "synthetic_input": "store.sources.increment_button.event.press",
                    "snapshot": app.snapshot(),
                    "frame": frame,
                    "wgpu_metadata": backend.metadata(),
                }))?,
            )?;
            Ok(VerifyReport {
                command: "verify native-wgpu --app-window".to_string(),
                results: vec![GateResult {
                    backend: Backend::NativeAppWindow,
                    example: "counter".to_string(),
                    passed,
                    frame_hash: frame.rgba_hash,
                    artifact_dir: dir,
                    message: if passed {
                        "passed app_window surface creation, compatible wgpu adapter/device, synthetic source dispatch, internal frame render, and readback".to_string()
                    } else {
                        "app_window smoke ran but did not produce the expected counter state/frame hash".to_string()
                    },
                }],
            })
        }
        Err(err) => {
            fs::write(dir.join("failure.txt"), err.to_string())?;
            Ok(VerifyReport {
                command: "verify native-wgpu --app-window".to_string(),
                results: vec![GateResult {
                    backend: Backend::NativeAppWindow,
                    example: "counter".to_string(),
                    passed: false,
                    frame_hash: None,
                    artifact_dir: dir,
                    message: err.to_string(),
                }],
            })
        }
    }
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
            backend.dispatch(
                app,
                state(
                    "store.sources.new_todo_input.text",
                    SourceValue::Text("Buy milk".to_string()),
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
                "store.todos_count",
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
        }
        "pong" | "arkanoid" => {
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
            backend.render_frame()?;
            expect(
                app.snapshot().values.get("interval_count"),
                json!(3),
                "interval_count",
            )?;
        }
        "todo_mvc" | "todo_mvc_physical" => {
            backend.dispatch(
                app,
                state(
                    "store.sources.new_todo_input.text",
                    SourceValue::Text("Buy milk".to_string()),
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
                "store.todos_count",
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
        }
        "pong" | "arkanoid" => {
            backend.dispatch(
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
