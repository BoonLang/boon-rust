use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .context("boon_examples must live under crates/")?;
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let manifest = root.join("examples").join("manifest.json");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let examples: Vec<String> = serde_json::from_str(
        &fs::read_to_string(&manifest)
            .with_context(|| format!("reading example manifest {}", manifest.display()))?,
    )
    .with_context(|| format!("parsing example manifest {}", manifest.display()))?;
    let inputs = examples
        .iter()
        .map(|name| {
            println!(
                "cargo:rerun-if-changed={}",
                root.join("examples").join(name).join("source.bn").display()
            );
            (
                name.as_str(),
                root.join("examples").join(name).join("source.bn"),
            )
        })
        .collect::<Vec<_>>();
    boon_codegen_rust::generate_examples_module(&inputs, &out_dir.join("generated_examples.rs"))?;
    Ok(())
}
