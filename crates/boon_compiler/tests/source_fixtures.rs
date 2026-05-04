use boon_compiler::compile_source;
use std::fs;
use std::path::PathBuf;

#[test]
fn maintained_examples_compile_and_match_source_snapshots() {
    let root = repo_root();
    for name in manifest_examples(&root) {
        let source_path = root.join("examples").join(&name).join("source.bn");
        let expected_path = root
            .join("examples")
            .join(&name)
            .join("expected.source_inventory.json");
        let source = fs::read_to_string(&source_path).expect("example source readable");
        let compiled = compile_source(&name, &source).expect("example compiles");
        assert!(
            compiled.hir.diagnostics.is_empty(),
            "compiled HIR for {name} must not contain ignored/raw syntax diagnostics: {:#?}",
            compiled.hir.diagnostics
        );
        let actual = serde_json::to_value(&compiled.sources).expect("inventory serializes");
        let expected: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&expected_path).expect("expected inventory readable"),
        )
        .expect("expected inventory parses");
        assert_eq!(actual, expected, "source inventory changed for {name}");

        let expected_path = root
            .join("examples")
            .join(&name)
            .join("expected.program.json");
        let actual = serde_json::to_value(&compiled.program).expect("program spec serializes");
        let expected: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&expected_path).expect("expected program spec readable"),
        )
        .expect("expected program spec parses");
        assert_eq!(actual, expected, "compiled program spec changed for {name}");

        let expected_path = root.join("examples").join(&name).join("expected.hir.json");
        let actual = serde_json::to_value(&compiled.hir).expect("HIR serializes");
        let expected: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&expected_path).expect("expected HIR snapshot readable"),
        )
        .expect("expected HIR snapshot parses");
        assert_eq!(actual, expected, "compiled HIR changed for {name}");

        let expected_path = root
            .join("examples")
            .join(&name)
            .join("expected.app_ir.json");
        let actual = serde_json::to_value(&compiled.app_ir).expect("app IR serializes");
        let expected: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&expected_path).expect("expected app IR snapshot readable"),
        )
        .expect("expected app IR snapshot parses");
        assert_eq!(actual, expected, "compiled app IR changed for {name}");

        let expected_path = root
            .join("examples")
            .join(&name)
            .join("expected.executable_ir.json");
        let actual =
            serde_json::to_value(&compiled.executable_ir).expect("executable IR serializes");
        let expected: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&expected_path).expect("expected executable IR snapshot readable"),
        )
        .expect("expected executable IR snapshot parses");
        assert_eq!(
            actual, expected,
            "compiled executable IR changed for {name}"
        );

        assert_render_tree_bindings_are_inventory_backed(&name, &compiled);
    }
}

fn assert_render_tree_bindings_are_inventory_backed(
    name: &str,
    compiled: &boon_compiler::CompiledModule,
) {
    let tree = compiled
        .app_ir
        .render_tree
        .as_ref()
        .unwrap_or_else(|| panic!("{name} must lower document into generic render tree"));
    let mut bindings = Vec::new();
    collect_render_bindings(tree, &mut bindings);
    assert!(
        !bindings.is_empty(),
        "{name} render tree should expose at least one source binding"
    );
    for binding in bindings {
        assert!(
            compiled.sources.entries.iter().any(
                |entry| entry.path == binding || entry.path.starts_with(&format!("{binding}."))
            ),
            "{name} render binding `{binding}` is not backed by source inventory {:#?}",
            compiled.sources.entries
        );
    }
}

fn collect_render_bindings<'a>(node: &'a boon_compiler::IrRenderNode, out: &mut Vec<&'a str>) {
    if let Some(path) = node.source_path.as_deref() {
        out.push(path);
    }
    for child in &node.children {
        collect_render_bindings(child, out);
    }
}

fn manifest_examples(root: &std::path::Path) -> Vec<String> {
    let path = root.join("examples").join("manifest.json");
    serde_json::from_str(&fs::read_to_string(&path).expect("example manifest readable"))
        .expect("example manifest parses")
}

#[test]
fn source_without_host_binding_is_a_compile_error() {
    let err = compile_source(
        "bad",
        r#"
store:
    sources:
        button:
            event:
                press: SOURCE
"#,
    )
    .expect_err("unbound SOURCE should fail");

    assert!(
        err.to_string()
            .contains("has no statically provable Element binding"),
        "unexpected error: {err}"
    );
}

#[test]
fn source_with_multiple_live_producers_is_a_compile_error() {
    let err = compile_source(
        "bad",
        r#"
store:
    sources:
        button:
            event:
                press: SOURCE

document:
    Document/new(
        root:
            Element/panel(
                children:
                    LIST {
                        Element/button(element: store.sources.button)
                        Element/button(element: store.sources.button)
                    }
            )
    )
"#,
    )
    .expect_err("duplicate source producer should fail");

    assert!(
        err.to_string()
            .contains("is bound by more than one Element producer"),
        "unexpected error: {err}"
    );
}

#[test]
fn source_bound_to_incompatible_element_contract_is_a_compile_error() {
    let err = compile_source(
        "bad",
        r#"
store:
    sources:
        input:
            text: SOURCE

document:
    Document/new(
        root:
            Element/button(element: store.sources.input)
    )
"#,
    )
    .expect_err("incompatible source binding should fail");

    assert!(
        err.to_string()
            .contains("has no statically provable Element binding"),
        "unexpected error: {err}"
    );
}

#[test]
fn source_path_read_without_bound_producer_is_a_compile_error() {
    let err = compile_source(
        "bad",
        r#"
store:
    sources:
        increment_button:
            event:
                press: SOURCE

counter:
    0 |> HOLD state {
        store.sources.increment_buton.event.press
        |> THEN { state + 1 }
    }

document:
    Document/new(
        root:
            Element/button(element: store.sources.increment_button)
    )
"#,
    )
    .expect_err("typo-like source read should fail");

    assert!(
        err.to_string().contains(
            "source path `store.sources.increment_buton.event.press` is read but no host/runtime producer is bound"
        ),
        "unexpected error: {err}"
    );
}

#[test]
fn unsupported_raw_expression_is_a_compile_error() {
    let err = compile_source(
        "bad",
        r#"
store:
    sources:
        button:
            event:
                press: SOURCE

bad_expression:
    state ?? 1

document:
    Document/new(
        root:
            Element/button(element: store.sources.button)
    )
"#,
    )
    .expect_err("unsupported raw syntax should fail");

    assert!(
        err.to_string().contains("unsupported Boon syntax"),
        "unexpected error: {err}"
    );
}

#[test]
fn unsupported_list_operation_is_a_compile_error() {
    let err = compile_source(
        "bad",
        r#"
store:
    sources:
        button:
            event:
                press: SOURCE

items:
    LIST {}
    |> List/State/new()

document:
    Document/new(
        root:
            Element/button(element: store.sources.button)
    )
"#,
    )
    .expect_err("unsupported List/* operation should fail");

    assert!(
        err.to_string()
            .contains("unsupported List operation `List/State/new`"),
        "unexpected error: {err}"
    );
}

#[test]
fn unsupported_math_operation_is_a_compile_error() {
    let err = compile_source(
        "bad",
        r#"
store:
    sources:
        button:
            event:
                press: SOURCE

formulas:
    functions:
        mean: Math/mean

document:
    Document/new(
        root:
            Element/button(element: store.sources.button)
    )
"#,
    )
    .expect_err("unsupported Math/* operation should fail");

    assert!(
        err.to_string()
            .contains("unsupported Math operation `Math/mean`"),
        "unexpected error: {err}"
    );
}

#[test]
fn counter_accumulator_lowers_to_generic_event_ir() {
    let root = repo_root();
    let source_path = root.join("examples").join("counter").join("source.bn");
    let source = fs::read_to_string(&source_path).expect("counter source readable");
    let compiled = compile_source("counter", &source).expect("counter compiles");

    assert_eq!(compiled.app_ir.state_cells.len(), 1);
    assert_eq!(compiled.app_ir.state_cells[0].path, "counter");
    assert_eq!(compiled.app_ir.event_handlers.len(), 1);
    assert_eq!(
        compiled.app_ir.event_handlers[0].source_path,
        "store.sources.increment_button.event.press"
    );
    let app_ir = serde_json::to_string(&compiled.app_ir).expect("app ir serializes");
    assert!(
        app_ir.contains("\"hold\"") && app_ir.contains("\"add\""),
        "counter app IR should preserve HOLD state + numeric step semantics: {app_ir}"
    );

    assert_eq!(compiled.executable_ir.state_slots.len(), 1);
    assert_eq!(compiled.executable_ir.state_slots[0].path, "counter");
    assert_eq!(compiled.executable_ir.source_handlers.len(), 1);
    assert_eq!(
        compiled.executable_ir.source_handlers[0].source_path,
        "store.sources.increment_button.event.press"
    );
    let executable_ir =
        serde_json::to_string(&compiled.executable_ir).expect("executable ir serializes");
    assert!(
        executable_ir.contains("\"set_state\"") && executable_ir.contains("\"add\""),
        "counter executable IR should preserve generic state update semantics: {executable_ir}"
    );
}

#[test]
fn todo_append_lowers_to_generic_list_event_ir() {
    let root = repo_root();
    let source_path = root.join("examples").join("todo_mvc").join("source.bn");
    let source = fs::read_to_string(&source_path).expect("todo source readable");
    let compiled = compile_source("todo_mvc", &source).expect("todo compiles");
    let app_ir = serde_json::to_string(&compiled.app_ir).expect("app ir serializes");

    assert!(
        app_ir.contains("\"collection_append_text_record\"")
            && app_ir.contains("\"text_field\":\"title\"")
            && app_ir.contains("\"field\":\"completed\""),
        "todo app IR should contain source-derived collection append/toggle fields: {app_ir}"
    );
    assert!(
        app_ir.contains("store.sources.new_todo_input.event.key_down.key")
            && app_ir.contains("\"source_tag_equals\"")
            && app_ir.contains("\"Enter\""),
        "todo append should be gated by the source key event predicate: {app_ir}"
    );
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("crate is under crates/")
        .to_path_buf()
}
