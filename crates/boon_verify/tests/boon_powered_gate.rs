use std::path::{Path, PathBuf};

#[test]
fn runtime_codegen_and_renderers_do_not_embed_example_business_logic() {
    let root = repo_root();
    let report = boon_verify::boon_powered_gate_report(&root, false)
        .expect("Boon-powered anti-cheat report should be constructible");
    assert!(
        report.passed,
        "Boon-powered gate failed: runtime/codegen/rendering files still embed example-specific \
         business logic or handcrafted example renderers.\n\n\
         Rust may implement generic parsing, lowering, turn execution, render IR application, \
         source dispatch, input plumbing, and verification. Example behavior and view structure \
         must come from examples/<name>/source.bn lowered through Boon IR/codegen.\n\n\
         Scanned files: {}\n\
         Handwritten Rust violations: {}\n\
         Genericity gaps: {}\n\
         Failed mutation probes: {}\n\
         Generated provenance passed: {}\n\n\
         First violations:\n{}\n\n\
         First genericity gaps:\n{}\n\n\
         Mutation probes:\n{}",
        report.scanned_files.len(),
        report.violations.len(),
        report.genericity_gaps.len(),
        report
            .mutation_probes
            .iter()
            .filter(|probe| !probe.passed)
            .count(),
        report.generated_provenance.passed,
        report
            .violations
            .iter()
            .take(80)
            .map(|violation| format!(
                "{}:{}:{} [{}] {}",
                violation.path,
                violation.line,
                violation.column,
                violation.check,
                violation.evidence
            ))
            .collect::<Vec<_>>()
            .join("\n"),
        report
            .genericity_gaps
            .iter()
            .take(120)
            .map(|gap| format!(
                "{}:{}:{} [{}] {}",
                gap.path, gap.line, gap.column, gap.category, gap.evidence
            ))
            .collect::<Vec<_>>()
            .join("\n"),
        report
            .mutation_probes
            .iter()
            .map(|probe| format!(
                "{} {} passed={} detail={}",
                probe.example, probe.mutation, probe.passed, probe.detail
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        report.genericity_complete,
        "genericity audit still reports fixed family-model gaps:\n{}",
        report
            .genericity_gaps
            .iter()
            .map(|gap| format!(
                "{}:{}:{} [{}] {}",
                gap.path, gap.line, gap.column, gap.category, gap.evidence
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        report.genericity_gaps.is_empty(),
        "genericity audit must not leave known fixed list/grid/dynamics model gaps"
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
