use boon_compiler::compile_source;
use boon_runtime::{BoonApp, CompiledApp, SourceBatch, SourceEmission, SourceValue};

#[test]
fn executable_when_and_number_calls_drive_state_generically() {
    let compiled = compile_source(
        "generic_controls",
        r#"
store:
    sources:
        keyboard:
            event:
                key_down:
                    key: SOURCE

position:
    50 |> HOLD value {
        store.sources.keyboard.event.key_down.key
        |> THEN {
            WHEN {
                ArrowLeft => Number/clamp(value: value - 8, min: 0, max: 100)
                ArrowRight => Number/clamp(value: value + 8, min: 0, max: 100)
                __ => value
            }
        }
    }

document:
    Document/new(
        root:
            Element/label(element: store.sources.keyboard)
    )
"#,
    )
    .expect("generic controls source compiles");
    let mut app = CompiledApp::new(compiled);
    app.mount();

    dispatch_key(&mut app, "ArrowLeft");
    assert_eq!(
        app.snapshot().values.get("position"),
        Some(&serde_json::json!(42))
    );

    dispatch_key(&mut app, "ArrowRight");
    assert_eq!(
        app.snapshot().values.get("position"),
        Some(&serde_json::json!(50))
    );

    dispatch_key(&mut app, "Escape");
    assert_eq!(
        app.snapshot().values.get("position"),
        Some(&serde_json::json!(50))
    );
}

#[test]
fn executable_handlers_for_one_event_use_the_same_pre_event_state() {
    let compiled = compile_source(
        "parallel_state",
        r#"
store:
    sources:
        tick:
            event:
                frame: SOURCE

x:
    1 |> HOLD old_x {
        store.sources.tick.event.frame |> THEN { old_x + y }
    }

y:
    10 |> HOLD old_y {
        store.sources.tick.event.frame |> THEN { old_y + x }
    }

document:
    Document/new(
        root:
            Element/label(element: store.sources.tick)
    )
"#,
    )
    .expect("parallel state source compiles");
    let mut app = CompiledApp::new(compiled);
    app.mount();

    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![SourceEmission {
            path: "store.sources.tick.event.frame".to_string(),
            value: SourceValue::EmptyRecord,
            owner_id: None,
            owner_generation: None,
        }],
    })
    .expect("dispatch succeeds");

    let snapshot = app.snapshot();
    assert_eq!(snapshot.values.get("x"), Some(&serde_json::json!(11)));
    assert_eq!(snapshot.values.get("y"), Some(&serde_json::json!(11)));
}

#[test]
fn nested_hold_state_paths_are_runtime_state_without_aliases() {
    let compiled = compile_source(
        "nested_state",
        r#"
store:
    sources:
        tick:
            event:
                frame: SOURCE

world:
    position:
        1 |> HOLD old_position {
            store.sources.tick.event.frame |> THEN { old_position + 2 }
        }

document:
    Document/new(
        root:
            Element/label(element: store.sources.tick)
    )
"#,
    )
    .expect("nested source compiles");
    let mut app = CompiledApp::new(compiled);
    app.mount();
    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![SourceEmission {
            path: "store.sources.tick.event.frame".to_string(),
            value: SourceValue::EmptyRecord,
            owner_id: None,
            owner_generation: None,
        }],
    })
    .expect("dispatch succeeds");

    assert_eq!(
        app.snapshot().values.get("world.position"),
        Some(&serde_json::json!(3))
    );
}

#[test]
fn pong_source_drives_control_and_frame_state_through_executable_ir() {
    let compiled = compile_source("pong", include_str!("../../../examples/pong/source.bn"))
        .expect("pong source compiles");
    assert!(
        compiled
            .executable_ir
            .state_slots
            .iter()
            .any(|slot| slot.path == "kinematics.body_x"),
        "pong body state must be generated from source, not hidden runtime state"
    );
    let mut app = CompiledApp::new(compiled);
    app.mount();

    dispatch(
        &mut app,
        "store.sources.paddle.event.key_down.key",
        SourceValue::Tag("ArrowUp".to_string()),
    );
    let after_key = app.snapshot();
    assert_eq!(
        after_key.values.get("kinematics.control_y"),
        Some(&serde_json::json!(42))
    );

    dispatch(
        &mut app,
        "store.sources.tick.event.frame",
        SourceValue::EmptyRecord,
    );
    let after_frame = app.snapshot();
    assert_eq!(
        after_frame.values.get("kinematics.frame"),
        Some(&serde_json::json!(1))
    );
    assert_ne!(
        after_frame.values.get("kinematics.body_x"),
        Some(&serde_json::json!(84)),
        "frame event should update the source-authored body position"
    );
}

#[test]
fn arkanoid_source_drives_control_and_contact_state_through_executable_ir() {
    let compiled = compile_source(
        "arkanoid",
        include_str!("../../../examples/arkanoid/source.bn"),
    )
    .expect("arkanoid source compiles");
    assert!(
        compiled
            .executable_ir
            .state_slots
            .iter()
            .any(|slot| slot.path == "kinematics.contact_field_live_count"),
        "arkanoid contact state must be generated from source, not hidden runtime state"
    );
    let mut app = CompiledApp::new(compiled);
    app.mount();

    dispatch(
        &mut app,
        "store.sources.paddle.event.key_down.key",
        SourceValue::Tag("ArrowRight".to_string()),
    );
    let after_key = app.snapshot();
    assert_eq!(
        after_key.values.get("kinematics.control_x"),
        Some(&serde_json::json!(58))
    );

    dispatch(
        &mut app,
        "store.sources.tick.event.frame",
        SourceValue::EmptyRecord,
    );
    let after_frame = app.snapshot();
    assert_eq!(
        after_frame.values.get("kinematics.frame"),
        Some(&serde_json::json!(1))
    );
    assert_ne!(
        after_frame.values.get("kinematics.body_y"),
        Some(&serde_json::json!(205)),
        "frame event should update the source-authored vertical body position"
    );
}

fn dispatch_key(app: &mut CompiledApp, key: &str) {
    dispatch(
        app,
        "store.sources.keyboard.event.key_down.key",
        SourceValue::Tag(key.to_string()),
    );
}

fn dispatch(app: &mut CompiledApp, path: &str, value: SourceValue) {
    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![SourceEmission {
            path: path.to_string(),
            value,
            owner_id: None,
            owner_generation: None,
        }],
    })
    .expect("dispatch succeeds");
}
