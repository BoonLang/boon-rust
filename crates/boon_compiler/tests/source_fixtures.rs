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

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("crate is under crates/")
        .to_path_buf()
}
