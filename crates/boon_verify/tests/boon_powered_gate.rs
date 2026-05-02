use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn runtime_codegen_and_renderers_do_not_embed_example_business_logic() {
    let root = repo_root();
    let scanned = [
        "crates/boon_runtime/src/compiled_app.rs",
        "crates/boon_backend_wgpu/src/lib.rs",
        "crates/boon_backend_ratatui/src/lib.rs",
        "crates/boon_runtime/src/lib.rs",
        "crates/boon_compiler/src/lib.rs",
    ];
    let forbidden = [
        "render_keyed_list",
        "render_grid",
        "render_frame_counter",
        "render_list_scene",
        "render_sheet_scene",
        "render_motion_scene",
        "GameModel",
        "MotionState",
        "GridModel",
        "ListWiring",
        "GridWiring",
        "MotionAxis",
        "ScalarCounterSpec",
        "TimerCounterSpec",
        "KeyedListSpec",
        "GridSpec",
        "MotionSpec",
        "SceneKind::ButtonCounter",
        "SceneKind::ClockCounter",
        "SceneKind::List",
        "SceneKind::Grid",
        "SceneKind::Motion",
        "advance_pong_frame",
        "advance_arkanoid_frame",
        "advance_game_frame",
        "advance_opposed_paddle_frame",
        "advance_brick_field_frame",
        "evaluate_formula",
        "formula_dependencies",
        "parse_cell_ref",
        "extract_initial_keyed_list_titles",
        "static_filter_names",
        "motion_spec",
        "draw_todomvc_preview",
        "draw_cells_preview",
        "draw_game_preview",
        "draw_collection_surface",
        "draw_table_surface",
        "draw_playfield_surface",
        "draw_action_value_surface",
        "draw_clock_value_surface",
        "draw_list_preview",
        "draw_table_preview",
        "draw_motion_preview",
        "draw_counter_preview",
        "draw_interval_preview",
        "scene_kind(frame_text)",
        "surface_kind(frame_text)",
        "parse_list_row",
        "row_preview_value",
        "frame_u32(frame_text",
        "frame_i32(frame_text",
        "parse_todo_line",
        "cell_preview_value",
        "motion_mode",
        "bricks_live",
        "paddle_y",
        "program.title.contains(\"Arkanoid\")",
        "program.title.contains(\"Pong\")",
        "new_todo_input",
        "toggle_all_checkbox",
        "clear_completed_button",
        "increment_button",
        "todos[*]",
        "cells[*]",
        "store.sources.tick",
        "store.sources.paddle",
        "store.sources.viewport",
        "TodoMVC",
        "Cells",
        "Pong",
        "Arkanoid",
        "todo_mvc",
        "arkanoid",
    ];

    let mut violations = Vec::new();
    for rel in scanned {
        let path = root.join(rel);
        let text = fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!(
                "failed to read {} for anti-cheat gate: {err}",
                path.display()
            )
        });
        for needle in forbidden {
            if text.contains(needle) {
                violations.push(format!("{rel}: contains `{needle}`"));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Boon-powered gate failed: runtime/codegen/rendering files still embed example-specific \
         business logic or handcrafted example renderers.\n\n\
         Rust may implement generic parsing, lowering, turn execution, render IR application, \
         source dispatch, input plumbing, and verification. Example behavior and view structure \
         must come from examples/<name>/source.bn lowered through Boon IR/codegen.\n\n\
         Violations:\n{}",
        violations.join("\n")
    );
}

fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("boon_verify crate is under crates/")
        .to_path_buf()
}
