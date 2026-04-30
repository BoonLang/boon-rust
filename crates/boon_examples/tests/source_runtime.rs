use boon_examples::app;
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
        app.snapshot().values.get("store.completed_todos_count"),
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
