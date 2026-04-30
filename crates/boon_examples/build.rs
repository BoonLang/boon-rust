use anyhow::{Context, Result};
use std::env;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .context("boon_examples must live under crates/")?;
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let examples = [
        "counter",
        "counter_hold",
        "interval",
        "interval_hold",
        "todo_mvc",
        "todo_mvc_physical",
        "cells",
        "pong",
        "arkanoid",
    ];
    let inputs = examples
        .iter()
        .map(|name| {
            println!(
                "cargo:rerun-if-changed={}",
                root.join("examples").join(name).join("source.bn").display()
            );
            (*name, root.join("examples").join(name).join("source.bn"))
        })
        .collect::<Vec<_>>();
    boon_codegen_rust::generate_examples_module(&inputs, &out_dir.join("generated_examples.rs"))?;
    Ok(())
}
