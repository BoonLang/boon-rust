use anyhow::Result;
use boon_runtime::{BoonApp, SourceBatch};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;

static OUTPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

#[derive(Clone, Debug, Deserialize)]
struct ScenarioInput {
    example: String,
    snapshot: Value,
    source_inventory: Value,
    frame_hash: Option<String>,
    timing: Value,
    replay: Vec<BrowserReplayStep>,
}

#[derive(Clone, Debug, Deserialize)]
enum BrowserReplayStep {
    Mount,
    Dispatch { batch: Value },
    AdvanceClock { millis: u64 },
}

#[derive(Clone, Debug, Serialize)]
struct RunnerOutput {
    ok: bool,
    scenarios: Vec<ScenarioProof>,
    errors: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct ScenarioProof {
    example: String,
    source_count: usize,
    expected_frame_hash: Option<String>,
    timing_passed: bool,
    snapshot_values: usize,
    snapshot_matches: bool,
    source_inventory_matches: bool,
    frame_text: String,
    errors: Vec<String>,
}

#[unsafe(no_mangle)]
pub extern "C" fn boon_alloc(len: usize) -> *mut u8 {
    let mut bytes = Vec::<u8>::with_capacity(len);
    let ptr = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    ptr
}

/// # Safety
///
/// `ptr` must have been returned by `boon_alloc` with the same `len`, and it
/// must not be used again after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn boon_dealloc(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        unsafe {
            let _ = Vec::from_raw_parts(ptr, 0, len);
        }
    }
}

/// # Safety
///
/// `ptr` must point to `len` initialized bytes containing a UTF-8 JSON array of
/// browser scenario inputs. The buffer must remain valid for the duration of the
/// call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn boon_run_scenarios(ptr: *const u8, len: usize) -> u32 {
    let result = run_scenarios(ptr, len);
    let ok = result.ok;
    let bytes = serde_json::to_vec(&result).unwrap_or_else(|err| {
        format!(
            r#"{{"ok":false,"scenarios":[],"errors":["failed to serialize wasm output: {err}"]}}"#
        )
        .into_bytes()
    });
    *OUTPUT.lock().expect("wasm output mutex poisoned") = bytes;
    u32::from(ok)
}

#[unsafe(no_mangle)]
pub extern "C" fn boon_output_ptr() -> *const u8 {
    OUTPUT.lock().expect("wasm output mutex poisoned").as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn boon_output_len() -> usize {
    OUTPUT.lock().expect("wasm output mutex poisoned").len()
}

fn run_scenarios(ptr: *const u8, len: usize) -> RunnerOutput {
    if ptr.is_null() {
        return RunnerOutput {
            ok: false,
            scenarios: Vec::new(),
            errors: vec!["input pointer is null".to_string()],
        };
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let inputs = match serde_json::from_slice::<Vec<ScenarioInput>>(bytes) {
        Ok(inputs) => inputs,
        Err(err) => {
            return RunnerOutput {
                ok: false,
                scenarios: Vec::new(),
                errors: vec![format!("failed to parse scenario input JSON: {err}")],
            };
        }
    };
    let scenarios = inputs.iter().map(prove_scenario).collect::<Vec<_>>();
    let ok = scenarios.iter().all(|proof| proof.errors.is_empty());
    RunnerOutput {
        ok,
        scenarios,
        errors: Vec::new(),
    }
}

fn prove_scenario(input: &ScenarioInput) -> ScenarioProof {
    let mut errors = Vec::new();
    let replay = replay_scenario(input);
    if let Err(err) = &replay {
        errors.push(err.to_string());
    }
    let (snapshot, source_inventory) = replay.unwrap_or((Value::Null, Value::Null));
    let source_count = source_inventory
        .get("entries")
        .and_then(|entries| entries.as_array())
        .map_or(0, Vec::len);
    if source_count == 0 {
        errors.push("wasm-generated source inventory is empty".to_string());
    }
    if input.frame_hash.as_deref().unwrap_or_default().is_empty() {
        errors.push("expected native frame hash is missing".to_string());
    }
    let snapshot_matches = snapshot == input.snapshot;
    if !snapshot_matches {
        errors
            .push("wasm-generated app snapshot differs from native scenario snapshot".to_string());
    }
    let source_inventory_matches = source_inventory == input.source_inventory;
    if !source_inventory_matches {
        errors.push(
            "wasm-generated source inventory differs from native source inventory".to_string(),
        );
    }
    let timing_passed = input
        .timing
        .get("passed")
        .and_then(|passed| passed.as_bool())
        .unwrap_or(true);
    if !timing_passed {
        errors.push("timing gate failed".to_string());
    }
    let snapshot_values = snapshot
        .get("values")
        .and_then(|values| values.as_object())
        .map_or(0, serde_json::Map::len);
    if snapshot_values == 0 {
        errors.push("wasm-generated snapshot has no values".to_string());
    }
    let frame_text = snapshot
        .get("frame_text")
        .and_then(|frame_text| frame_text.as_str())
        .unwrap_or_default()
        .to_string();
    if frame_text.trim().is_empty() {
        errors.push("wasm-generated frame text is empty".to_string());
    }
    ScenarioProof {
        example: input.example.clone(),
        source_count,
        expected_frame_hash: input.frame_hash.clone(),
        timing_passed,
        snapshot_values,
        snapshot_matches,
        source_inventory_matches,
        frame_text,
        errors,
    }
}

fn replay_scenario(input: &ScenarioInput) -> Result<(Value, Value)> {
    let mut app = boon_examples::app(&input.example)?;
    for step in &input.replay {
        match step {
            BrowserReplayStep::Mount => {
                app.mount();
            }
            BrowserReplayStep::Dispatch { batch } => {
                let batch = serde_json::from_value::<SourceBatch>(batch.clone())?;
                dispatch(&mut app, batch)?;
            }
            BrowserReplayStep::AdvanceClock { millis } => {
                app.advance_time(std::time::Duration::from_millis(*millis));
            }
        }
    }
    Ok((
        serde_json::to_value(app.snapshot())?,
        serde_json::to_value(app.source_inventory())?,
    ))
}

fn dispatch(app: &mut impl BoonApp, batch: SourceBatch) -> Result<()> {
    app.dispatch_batch(batch)?;
    Ok(())
}
