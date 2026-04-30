use anyhow::Result;
use boon_compiler::compile_source;
use std::fs;
use std::path::Path;

pub fn generate_manifest(example_name: &str, source_path: &Path, output_path: &Path) -> Result<()> {
    let source = fs::read_to_string(source_path)?;
    let compiled = compile_source(example_name, &source)?;
    let json = serde_json::to_string_pretty(&compiled.sources)?;
    fs::write(output_path, json)?;
    Ok(())
}

pub fn generate_program_spec(
    example_name: &str,
    source_path: &Path,
    output_path: &Path,
) -> Result<()> {
    let source = fs::read_to_string(source_path)?;
    let compiled = compile_source(example_name, &source)?;
    let json = serde_json::to_string_pretty(&compiled.program)?;
    fs::write(output_path, json)?;
    Ok(())
}

pub fn generate_examples_module(
    examples: &[(&str, impl AsRef<Path>)],
    output_path: &Path,
) -> Result<()> {
    let mut code = String::new();
    code.push_str(
        r#"use anyhow::{Result, bail};
use boon_compiler::{ProgramSpec, compile_source};
use boon_render_ir::{HostPatch, NodeId, NodeKind};
use boon_runtime::{
    AppSnapshot, BoonApp, FakeClock, SourceBatch, SourceEmission, SourceInventory, SourceValue,
    StateDelta, TurnId, TurnMetrics, TurnResult,
};
use boon_shape::Shape;
use boon_source::SourceOwner;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

"#,
    );
    code.push_str("pub const EXAMPLES: &[&str] = &[\n");
    for (name, _) in examples {
        code.push_str(&format!("    {name:?},\n"));
    }
    code.push_str("];\n\n");
    code.push_str("#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]\n");
    code.push_str("pub struct ExampleDefinition {\n");
    code.push_str("    pub name: &'static str,\n");
    code.push_str("    pub source: &'static str,\n");
    code.push_str("}\n\n");
    code.push_str("const DEFINITIONS: &[ExampleDefinition] = &[\n");
    for (name, source_path) in examples {
        let source = fs::read_to_string(source_path.as_ref())?;
        let compiled = compile_source(name, &source)?;
        code.push_str(&format!(
            "    ExampleDefinition {{ name: {name:?}, source: {} }},\n",
            rust_string_literal(&source)
        ));
        code.push_str(&format!(
            "    // compiled source slots: {}\n",
            compiled.sources.entries.len()
        ));
    }
    code.push_str("];\n\n");
    code.push_str(
        r#"pub fn list_examples() -> &'static [&'static str] {
    EXAMPLES
}

pub fn definition(name: &str) -> Result<ExampleDefinition> {
    DEFINITIONS
        .iter()
        .find(|definition| definition.name == name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("unknown example `{name}`"))
}

pub fn source_inventory(name: &str) -> Result<SourceInventory> {
    let def = definition(name)?;
    Ok(compile_source(name, def.source)?.sources)
}

pub fn app(name: &str) -> Result<ExampleApp> {
    let def = definition(name)?;
    Ok(ExampleApp::new(compile_source(name, def.source)?))
}

"#,
    );
    code.push_str(include_str!("example_runtime_template.rs"));
    fs::write(output_path, code)?;
    Ok(())
}

fn rust_string_literal(value: &str) -> String {
    format!("{value:?}")
}
