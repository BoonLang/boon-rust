use boon_verify::verify_ratatui;

#[test]
fn ratatui_gate_covers_all_examples() {
    let dir = tempfile_dir();
    let report = verify_ratatui(&dir, false).expect("ratatui verification passes");
    assert_eq!(report.results.len(), 9);
    assert!(report.results.iter().all(|result| result.passed));
}

fn tempfile_dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("boon-rust-ratatui-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}
