use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn runtime_codegen_and_renderers_do_not_embed_example_business_logic() {
    let root = repo_root();
    let scanned = [
        "crates/boon_codegen_rust/src/example_runtime_template.rs",
        "crates/boon_backend_wgpu/src/lib.rs",
        "crates/boon_backend_ratatui/src/lib.rs",
        "crates/boon_runtime/src/lib.rs",
        "crates/boon_compiler/src/lib.rs",
    ];
    let forbidden = [
        "render_keyed_list",
        "render_grid",
        "render_frame_counter",
        "GameModel",
        "advance_pong_frame",
        "advance_arkanoid_frame",
        "draw_todomvc_preview",
        "draw_cells_preview",
        "draw_game_preview",
        "parse_todo_line",
        "cell_preview_value",
        "program.title.contains(\"Arkanoid\")",
        "program.title.contains(\"Pong\")",
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
