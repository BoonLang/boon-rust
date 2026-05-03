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
fn counter_accumulator_lowers_to_generic_event_ir() {
    let root = repo_root();
    let source_path = root.join("examples").join("counter").join("source.bn");
    let source = fs::read_to_string(&source_path).expect("counter source readable");
    let compiled = compile_source("counter", &source).expect("counter compiles");

    assert_eq!(compiled.app_ir.state_cells.len(), 1);
    assert_eq!(compiled.app_ir.state_cells[0].path, "scalar_value");
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
}

#[test]
fn todo_append_lowers_to_generic_list_event_ir() {
    let root = repo_root();
    let source_path = root.join("examples").join("todo_mvc").join("source.bn");
    let source = fs::read_to_string(&source_path).expect("todo source readable");
    let compiled = compile_source("todo_mvc", &source).expect("todo compiles");
    let app_ir = serde_json::to_string(&compiled.app_ir).expect("app ir serializes");

    assert!(
        app_ir.contains("\"list_append_text\""),
        "todo app IR should contain a generic list append effect: {app_ir}"
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
