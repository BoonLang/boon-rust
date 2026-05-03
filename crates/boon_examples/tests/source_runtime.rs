use boon_compiler::compile_source;
use boon_examples::{CompiledApp, app, definition};
use boon_runtime::{BoonApp, SourceBatch, SourceEmission, SourceValue};

#[test]
fn source_batch_applies_state_updates_before_events() {
    let mut app = app("todo_mvc").expect("todo app");
    app.dispatch_batch(SourceBatch {
        state_updates: vec![emission(
            "store.sources.new_todo_input.text",
            SourceValue::Text("Write source batch test".to_string()),
        )],
        events: vec![emission(
            "store.sources.new_todo_input.event.key_down.key",
            SourceValue::Tag("Enter".to_string()),
        )],
    })
    .expect("batch dispatch succeeds");

    assert_eq!(
        app.snapshot().values.get("store.todos_count"),
        Some(&serde_json::json!(3))
    );
}

#[test]
fn source_dispatch_rejects_unknown_paths_and_shapes() {
    let mut app = app("counter").expect("counter app");
    let unknown = app
        .dispatch_batch(SourceBatch {
            state_updates: Vec::new(),
            events: vec![emission(
                "store.sources.increment_buton.event.press",
                SourceValue::EmptyRecord,
            )],
        })
        .expect_err("unknown SOURCE path should fail")
        .to_string();
    assert!(unknown.contains("unknown SOURCE path"), "{unknown}");

    let wrong_shape = app
        .dispatch_batch(SourceBatch {
            state_updates: Vec::new(),
            events: vec![emission(
                "store.sources.increment_button.event.press",
                SourceValue::Text("bad".to_string()),
            )],
        })
        .expect_err("shape mismatch should fail")
        .to_string();
    assert!(
        wrong_shape.contains("expected EmptyRecord"),
        "{wrong_shape}"
    );
}

#[test]
fn dynamic_sources_require_live_owner_generation() {
    let mut app = app("todo_mvc").expect("todo app");
    let missing_owner = app
        .dispatch_batch(SourceBatch {
            state_updates: Vec::new(),
            events: vec![emission(
                "todos[*].sources.checkbox.event.click",
                SourceValue::EmptyRecord,
            )],
        })
        .expect_err("dynamic SOURCE without owner metadata should fail")
        .to_string();
    assert!(
        missing_owner.contains("missing owner_id"),
        "{missing_owner}"
    );

    let stale_generation = app
        .dispatch_batch(SourceBatch {
            state_updates: Vec::new(),
            events: vec![dynamic_emission(
                "todos[*].sources.checkbox.event.click",
                "1",
                1,
                SourceValue::EmptyRecord,
            )],
        })
        .expect_err("stale generation should fail")
        .to_string();
    assert!(
        stale_generation.contains("stale dynamic SOURCE"),
        "{stale_generation}"
    );

    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![dynamic_emission(
            "todos[*].sources.checkbox.event.click",
            "1",
            0,
            SourceValue::EmptyRecord,
        )],
    })
    .expect("live dynamic owner generation succeeds");
    assert_eq!(
        app.snapshot().values.get("store.marked_todos_count"),
        Some(&serde_json::json!(1))
    );
}

#[test]
fn source_state_updates_emit_render_patches() {
    let mut app = app("todo_mvc").expect("todo app");
    let turns = app
        .dispatch_batch(SourceBatch {
            state_updates: vec![emission(
                "store.sources.new_todo_input.text",
                SourceValue::Text("visible input".to_string()),
            )],
            events: Vec::new(),
        })
        .expect("state-only batch succeeds");
    assert_eq!(turns.len(), 1);
    assert!(app.snapshot().frame_text.contains("visible input"));
}

#[test]
fn view_metadata_comes_from_boon_source() {
    let source = definition("todo_mvc")
        .expect("todo_mvc definition")
        .source
        .replace("What needs to be done?", "Source-owned placeholder");
    let app = CompiledApp::new(
        compile_source("todo_mvc_custom_placeholder", &source).expect("source compiles"),
    );

    let frame = app.snapshot().frame_text;
    assert!(frame.contains("Source-owned placeholder"), "{frame}");
    assert!(
        !frame.contains("What needs to be done?"),
        "the runtime must not preserve old UI copy through a Rust fallback: {frame}"
    );
}

#[test]
fn cells_recompute_dirty_dependents_and_cycles_deterministically() {
    let mut app = app("cells").expect("cells app");
    for (owner, value) in [
        ("A1", "1"),
        ("A2", "2"),
        ("A3", "3"),
        ("B1", "=add(A1, A2)"),
        ("B2", "=sum(A1:A3)"),
    ] {
        app.dispatch_batch(SourceBatch {
            state_updates: vec![dynamic_emission(
                "cells[*].sources.editor.text",
                owner,
                0,
                SourceValue::Text(value.to_string()),
            )],
            events: Vec::new(),
        })
        .expect("cell update succeeds");
    }
    assert_eq!(
        app.snapshot().values.get("cells.B1"),
        Some(&serde_json::json!("3"))
    );
    assert_eq!(
        app.snapshot().values.get("cells.B2"),
        Some(&serde_json::json!("6"))
    );

    app.dispatch_batch(SourceBatch {
        state_updates: vec![dynamic_emission(
            "cells[*].sources.editor.text",
            "A2",
            0,
            SourceValue::Text("5".to_string()),
        )],
        events: Vec::new(),
    })
    .expect("dependent update succeeds");
    assert_eq!(
        app.snapshot().values.get("cells.B1"),
        Some(&serde_json::json!("6"))
    );
    assert_eq!(
        app.snapshot().values.get("cells.B2"),
        Some(&serde_json::json!("9"))
    );

    app.dispatch_batch(SourceBatch {
        state_updates: vec![dynamic_emission(
            "cells[*].sources.editor.text",
            "A1",
            0,
            SourceValue::Text("=add(A1, A2)".to_string()),
        )],
        events: Vec::new(),
    })
    .expect("cycle update is represented deterministically");
    assert_eq!(
        app.snapshot().values.get("cells.A1"),
        Some(&serde_json::json!("#CYCLE"))
    );
}

#[test]
fn behavior_changes_when_source_reducer_expression_is_removed() {
    let source = definition("counter")
        .expect("counter definition")
        .source
        .replace("state + 1", "state");
    let mut app =
        CompiledApp::new(compile_source("counter_no_increment", &source).expect("source compiles"));

    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![emission(
            "store.sources.increment_button.event.press",
            SourceValue::EmptyRecord,
        )],
    })
    .expect("counter dispatch succeeds");

    assert_eq!(
        app.snapshot().values.get("scalar_value"),
        Some(&serde_json::json!(0)),
        "removing the Boon increment expression must change behavior"
    );
}

#[test]
fn behavior_changes_when_source_list_append_pipeline_is_removed() {
    let source = definition("todo_mvc")
        .expect("todo_mvc definition")
        .source
        .replace("|> List/append(", "|> Disabled_append(");
    let mut app =
        CompiledApp::new(compile_source("todo_mvc_no_append", &source).expect("source compiles"));

    app.dispatch_batch(SourceBatch {
        state_updates: vec![emission(
            "store.sources.new_todo_input.text",
            SourceValue::Text("Should not append".to_string()),
        )],
        events: vec![emission(
            "store.sources.new_todo_input.event.key_down.key",
            SourceValue::Tag("Enter".to_string()),
        )],
    })
    .expect("todo dispatch succeeds");

    assert_ne!(
        app.snapshot().values.get("store.todos_count"),
        Some(&serde_json::json!(3)),
        "removing the Boon List/append pipeline must not preserve append behavior through a fallback"
    );
}

#[test]
fn behavior_changes_when_source_formula_function_is_removed() {
    let source = definition("cells")
        .expect("cells definition")
        .source
        .replace("        sum: Math/sum\n", "");
    let mut app =
        CompiledApp::new(compile_source("cells_without_sum", &source).expect("source compiles"));

    for (owner, value) in [("A1", "1"), ("A2", "2"), ("B1", "=sum(A1:A2)")] {
        app.dispatch_batch(SourceBatch {
            state_updates: vec![dynamic_emission(
                "cells[*].sources.editor.text",
                owner,
                0,
                SourceValue::Text(value.to_string()),
            )],
            events: Vec::new(),
        })
        .expect("cell update succeeds");
    }

    assert_eq!(
        app.snapshot().values.get("cells.B1"),
        Some(&serde_json::json!("#ERR")),
        "removing Math/sum from Boon source must disable sum formulas"
    );
}

#[test]
fn behavior_changes_when_source_contact_field_is_removed() {
    let source = definition("arkanoid")
        .expect("arkanoid definition")
        .source
        .replace("    contact_field:", "    disabled_contact_field:");
    let mut app = CompiledApp::new(
        compile_source("arkanoid_without_contact_field", &source).expect("source compiles"),
    );

    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![emission(
            "store.sources.tick.event.frame",
            SourceValue::EmptyRecord,
        )],
    })
    .expect("frame dispatch succeeds");

    assert_eq!(
        app.snapshot().values.get("kinematics.contact_field_cols"),
        Some(&serde_json::json!(0)),
        "removing the contact field from Boon source must remove contact-field behavior"
    );
}

fn emission(path: &str, value: SourceValue) -> SourceEmission {
    SourceEmission {
        path: path.to_string(),
        value,
        owner_id: None,
        owner_generation: None,
    }
}

fn dynamic_emission(
    path: &str,
    owner_id: &str,
    owner_generation: u32,
    value: SourceValue,
) -> SourceEmission {
    SourceEmission {
        path: path.to_string(),
        value,
        owner_id: Some(owner_id.to_string()),
        owner_generation: Some(owner_generation),
    }
}
