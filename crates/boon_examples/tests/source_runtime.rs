use boon_compiler::compile_source;
use boon_examples::{CompiledApp, app, definition, executable_ir};
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
fn generated_examples_expose_source_derived_executable_ir() {
    let counter = executable_ir("counter").expect("counter executable IR is generated");
    assert_eq!(counter.state_slots.len(), 1);
    assert_eq!(counter.state_slots[0].path, "counter");
    assert_eq!(counter.source_handlers.len(), 1);
    assert_eq!(
        counter.source_handlers[0].source_path,
        "store.sources.increment_button.event.press"
    );

    let interval = executable_ir("interval").expect("interval executable IR is generated");
    assert_eq!(interval.state_slots.len(), 1);
    assert_eq!(interval.state_slots[0].path, "ticks");
    assert_eq!(interval.source_handlers.len(), 1);
    assert_eq!(
        interval.source_handlers[0].source_path,
        "store.sources.clock.event.tick"
    );
}

#[test]
fn counter_and_interval_render_from_generic_boon_scene_tree() {
    let mut counter = app("counter").expect("counter app");
    counter.mount();
    let counter_frame = counter.snapshot().frame_text;
    assert!(
        counter_frame.contains("surface: generic_scene"),
        "counter should render through generic Boon scene tree: {counter_frame}"
    );
    assert!(
        !counter_frame.contains("button-scalar"),
        "counter must not use the scalar fallback renderer: {counter_frame}"
    );

    let mut interval = app("interval").expect("interval app");
    interval.mount();
    let interval_frame = interval.snapshot().frame_text;
    assert!(
        interval_frame.contains("surface: generic_scene"),
        "interval should render through generic Boon scene tree: {interval_frame}"
    );
    assert!(
        !interval_frame.contains("clock-scalar"),
        "interval must not use the clock fallback renderer: {interval_frame}"
    );
}

#[test]
fn counter_dispatch_uses_executable_ir_without_legacy_app_ir_handlers() {
    let source = definition("counter")
        .expect("counter definition")
        .source
        .to_string();
    let mut compiled = compile_source("counter_without_legacy_handlers", &source)
        .expect("counter source compiles");
    compiled.app_ir.event_handlers.clear();
    let mut app = CompiledApp::new(compiled);

    for _ in 0..2 {
        app.dispatch_batch(SourceBatch {
            state_updates: Vec::new(),
            events: vec![emission(
                "store.sources.increment_button.event.press",
                SourceValue::EmptyRecord,
            )],
        })
        .expect("counter dispatch uses executable IR");
    }

    assert_eq!(
        app.snapshot().values.get("counter"),
        Some(&serde_json::json!(2))
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
fn ad_hoc_grid_example_uses_source_owned_root() {
    let source = r#"
store:
    sources:
        viewport:
            event:
                key_down:
                    key: SOURCE

sheet:
    sources:
        editor:
            text: SOURCE
            event:
                key_down:
                    key: SOURCE

rows:
    List/range(from: 1, to: 3)

columns:
    List/range(from: 1, to: 3)

expressions:
    functions:
        add: Math/add
        sum: Math/sum

grid:
    rows |> List/map(row, new:
        columns |> List/map(column, new:
            [
                row: row
                column: column
                sources: sheet.sources
            ]
        )
    )

document:
    Document/new(
        root:
            Element/grid(
                element: store.sources.viewport
                rows: rows
                columns: columns
                expressions: expressions
                cells:
                    grid |> List/map(row, new:
                        row |> List/map(cell, new:
                            Element/panel(
                                children:
                                    LIST {
                                        Element/text_input(element: cell.sources.editor)
                                    }
                            )
                        )
                    )
            )
    )
"#;
    let mut app = CompiledApp::new(
        compile_source("ad_hoc_grid_probe", source).expect("ad hoc grid source compiles"),
    );

    for (owner, value) in [("A1", "2"), ("A2", "5"), ("B1", "=add(A1, A2)")] {
        app.dispatch_batch(SourceBatch {
            state_updates: vec![dynamic_emission(
                "sheet[*].sources.editor.text",
                owner,
                0,
                SourceValue::Text(value.to_string()),
            )],
            events: Vec::new(),
        })
        .expect("ad hoc grid update succeeds");
    }

    assert_eq!(
        app.snapshot().values.get("sheet.B1"),
        Some(&serde_json::json!("7"))
    );
    assert!(
        !app.snapshot().values.contains_key("cells.B1"),
        "ad hoc grid state must use the source-owned root, not the maintained cells example root"
    );
}

#[test]
fn ad_hoc_physics_example_uses_generic_geometry_runtime() {
    let source = r#"
store:
    sources:
        tick:
            event:
                frame: SOURCE

physics:
    body_x:
        0 |> HOLD body_x {
            store.sources.tick.event.frame |> THEN { body_x + 5 }
        }
    hit_count:
        0 |> HOLD hit_count {
            store.sources.tick.event.frame
            |> THEN {
                Geometry/intersects(ax: physics.body_x + 5, ay: 10, aw: 10, ah: 10, bx: 8, by: 10, bw: 10, bh: 10)
                |> WHEN { True => hit_count + 1 False => hit_count }
            }
        }

document:
    Document/new(
        root:
            Element/panel(
                children:
                    LIST {
                        Element/label(element: store.sources.tick)
                        Element/text(text: physics.body_x)
                        Element/text(text: physics.hit_count)
                        Element/rect(x: physics.body_x, y: 10, width: 10, height: 10, color: TEXT { #ffffff })
                        Element/rect(x: 8, y: 10, width: 10, height: 10, color: TEXT { #55d4e6 })
                    }
            )
    )
"#;
    let mut app = CompiledApp::new(
        compile_source("ad_hoc_physics_probe", source).expect("ad hoc physics source compiles"),
    );

    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![emission(
            "store.sources.tick.event.frame",
            SourceValue::EmptyRecord,
        )],
    })
    .expect("ad hoc physics frame dispatch succeeds");

    assert_eq!(
        app.snapshot().values.get("physics.body_x"),
        Some(&serde_json::json!(5))
    );
    assert_eq!(
        app.snapshot().values.get("physics.hit_count"),
        Some(&serde_json::json!(1))
    );
    assert!(
        !app.snapshot().values.contains_key("kinematics.body_x"),
        "ad hoc physics state must use the source-owned root, not a maintained game family"
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

    assert_ne!(
        app.snapshot().values.get("counter"),
        Some(&serde_json::json!(1)),
        "removing the Boon increment expression must remove the increment behavior"
    );
}

#[test]
fn interval_tick_event_runs_through_generic_event_ir() {
    let mut app = app("interval").expect("interval app");
    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![emission(
            "store.sources.clock.event.tick",
            SourceValue::EmptyRecord,
        )],
    })
    .expect("tick event dispatch succeeds");

    assert_eq!(
        app.snapshot().values.get("ticks"),
        Some(&serde_json::json!(1))
    );
}

#[test]
fn todo_list_controls_run_through_generic_list_ir() {
    let mut app = app("todo_mvc").expect("todo app");

    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![dynamic_emission(
            "todos[*].sources.checkbox.event.click",
            "1",
            0,
            SourceValue::EmptyRecord,
        )],
    })
    .expect("dynamic checkbox event succeeds");
    assert_eq!(
        app.snapshot().values.get("store.completed_todos_count"),
        Some(&serde_json::json!(1))
    );

    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![emission(
            "store.sources.filter_completed.event.press",
            SourceValue::EmptyRecord,
        )],
    })
    .expect("filter event succeeds");
    assert_eq!(
        app.snapshot().values.get("view.selector"),
        Some(&serde_json::json!("filter_completed"))
    );

    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![dynamic_emission(
            "todos[*].sources.remove_button.event.press",
            "1",
            0,
            SourceValue::EmptyRecord,
        )],
    })
    .expect("dynamic remove event succeeds");
    assert_eq!(
        app.snapshot().values.get("store.todos_count"),
        Some(&serde_json::json!(1))
    );

    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![emission(
            "store.sources.toggle_all_checkbox.event.click",
            SourceValue::EmptyRecord,
        )],
    })
    .expect("toggle-all event succeeds");
    assert_eq!(
        app.snapshot().values.get("store.completed_todos_count"),
        Some(&serde_json::json!(1))
    );

    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![emission(
            "store.sources.clear_completed_button.event.press",
            SourceValue::EmptyRecord,
        )],
    })
    .expect("clear completed event succeeds");
    assert_eq!(
        app.snapshot().values.get("store.todos_count"),
        Some(&serde_json::json!(0))
    );
}

#[test]
fn ad_hoc_list_example_uses_generic_append_runtime() {
    let source = r#"
store:
    sources:
        entry:
            text: SOURCE
            event:
                key_down:
                    key: SOURCE
                change: SOURCE

FUNCTION row(title) {
    [
        sources:
            remove_button:
                event:
                    press: SOURCE
        title: title
    ]
}

title_to_add:
    store.sources.entry.event.key_down.key
    |> WHEN {
        Enter => BLOCK {
            trimmed: store.sources.entry.text |> Text/trim()
            trimmed |> Text/is_not_empty() |> WHEN { True => trimmed False => SKIP }
        }
        __ => SKIP
    }

items:
    LIST {}
    |> List/append(item: title_to_add |> row(title: PASSED))
    |> List/remove(item, on: item.sources.remove_button.event.press)

document:
    Document/new(
        root:
            Element/panel(
                children:
                    LIST {
                        Element/text_input(element: store.sources.entry)
                        items |> List/map(item, new:
                            Element/panel(
                                children:
                                    LIST {
                                        Element/button(element: item.sources.remove_button)
                                    }
                            )
                        )
                    }
            )
    )
"#;
    let mut app = CompiledApp::new(
        compile_source("ad_hoc_list_probe", source).expect("ad hoc list source compiles"),
    );

    app.dispatch_batch(SourceBatch {
        state_updates: vec![emission(
            "store.sources.entry.text",
            SourceValue::Text("  generic item  ".to_string()),
        )],
        events: vec![emission(
            "store.sources.entry.event.key_down.key",
            SourceValue::Tag("Enter".to_string()),
        )],
    })
    .expect("generic list append dispatch succeeds");

    assert_eq!(
        app.snapshot().values.get("store.items_count"),
        Some(&serde_json::json!(1))
    );
    assert_eq!(
        app.snapshot().values.get("store.items_titles"),
        Some(&serde_json::json!(["generic item"]))
    );

    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![dynamic_emission(
            "items[*].sources.remove_button.event.press",
            "1",
            0,
            SourceValue::EmptyRecord,
        )],
    })
    .expect("generic list remove dispatch succeeds");

    assert_eq!(
        app.snapshot().values.get("store.items_count"),
        Some(&serde_json::json!(0))
    );
}

#[test]
fn ad_hoc_selector_record_filters_dynamic_rows_without_view_name() {
    let source = r#"
store:
    sources:
        filter_open:
            event:
                press: SOURCE
        filter_done:
            event:
                press: SOURCE

FUNCTION row(title, done) {
    [
        sources:
            remove_button:
                event:
                    press: SOURCE
        title: title
        done: done
    ]
}

items:
    LIST {
        row(title: TEXT { Open item }, done: False)
        row(title: TEXT { Done item }, done: True)
    }

filters:
    selectors:
        filter_open:
            predicate:
                field: done
                equals: False
        filter_done:
            predicate:
                field: done
                equals: True

document:
    Document/new(
        root:
            Element/panel(
                children:
                    LIST {
                        Element/button(element: store.sources.filter_open)
                        Element/button(element: store.sources.filter_done)
                        items |> List/map(item, new:
                            Element/panel(
                                children:
                                    LIST {
                                        Element/button(element: item.sources.remove_button)
                                    }
                            )
                        )
                    }
            )
    )
"#;
    let compiled =
        compile_source("ad_hoc_selector_probe", source).expect("ad hoc selector source compiles");
    println!(
        "{}",
        serde_json::to_string_pretty(&compiled.app_ir).unwrap()
    );
    let mut app = CompiledApp::new(compiled);
    println!("{:#?}", app.snapshot().values);

    assert_eq!(
        app.snapshot().values.get("filters.selector"),
        Some(&serde_json::json!("filter_open"))
    );
    assert_eq!(
        app.snapshot().values.get("store.visible_items_ids"),
        Some(&serde_json::json!([1]))
    );

    app.dispatch_batch(SourceBatch {
        state_updates: Vec::new(),
        events: vec![emission(
            "store.sources.filter_done.event.press",
            SourceValue::EmptyRecord,
        )],
    })
    .expect("generic selector dispatch succeeds");

    assert_eq!(
        app.snapshot().values.get("filters.selector"),
        Some(&serde_json::json!("filter_done"))
    );
    assert_eq!(
        app.snapshot().values.get("store.visible_items_ids"),
        Some(&serde_json::json!([2]))
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
        "removing Math/sum from Boon source must disable sum expressions"
    );
}

#[test]
fn behavior_changes_when_source_contact_columns_change() {
    let source = definition("arkanoid")
        .expect("arkanoid definition")
        .source
        .replace(
            "        12 |> HOLD contact_field_cols",
            "        0 |> HOLD contact_field_cols",
        );
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
        "changing contact-field columns in Boon source must change contact-field behavior"
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
