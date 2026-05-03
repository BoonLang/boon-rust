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

pub fn generate_hir_snapshot(
    example_name: &str,
    source_path: &Path,
    output_path: &Path,
) -> Result<()> {
    let source = fs::read_to_string(source_path)?;
    let compiled = compile_source(example_name, &source)?;
    let json = serde_json::to_string_pretty(&compiled.hir)?;
    fs::write(output_path, json)?;
    Ok(())
}

pub fn generate_app_ir_snapshot(
    example_name: &str,
    source_path: &Path,
    output_path: &Path,
) -> Result<()> {
    let source = fs::read_to_string(source_path)?;
    let compiled = compile_source(example_name, &source)?;
    let json = serde_json::to_string_pretty(&compiled.app_ir)?;
    fs::write(output_path, json)?;
    Ok(())
}

pub fn generate_examples_module(
    examples: &[(&str, impl AsRef<Path>)],
    output_path: &Path,
) -> Result<()> {
    let mut code = String::new();
    code.push_str(
        r#"use anyhow::Result;
use boon_compiler::compile_source;
use boon_runtime::SourceInventory;
pub use boon_runtime::{CompiledApp, ExampleApp};
use serde::{Deserialize, Serialize};

"#,
    );
    let mut compiled_examples = Vec::new();
    for (name, source_path) in examples {
        let source = fs::read_to_string(source_path.as_ref())?;
        let compiled = compile_source(name, &source)?;
        compiled_examples.push((name.to_string(), source, compiled));
    }
    code.push_str("pub const SOURCE_SHA256: &[(&str, &str)] = &[\n");
    for (name, _, compiled) in &compiled_examples {
        code.push_str(&format!(
            "    ({name:?}, {:?}),\n",
            compiled.provenance.source_sha256
        ));
    }
    code.push_str("];\n\n");
    code.push_str("pub const IR_SHA256: &[(&str, &str)] = &[\n");
    for (name, _, compiled) in &compiled_examples {
        code.push_str(&format!(
            "    ({name:?}, {:?}),\n",
            compiled.provenance.hir_sha256
        ));
    }
    code.push_str("];\n\n");
    code.push_str(
        "#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]\n\
         pub struct SourceSpanRecord {\n\
             pub example: &'static str,\n\
             pub kind: &'static str,\n\
             pub path: &'static str,\n\
             pub line: usize,\n\
             pub column: usize,\n\
         }\n\n",
    );
    code.push_str("pub const SOURCE_SPANS: &[SourceSpanRecord] = &[\n");
    for (name, _, compiled) in &compiled_examples {
        for span in &compiled.provenance.source_spans {
            code.push_str(&format!(
                "    SourceSpanRecord {{ example: {name:?}, kind: {:?}, path: {:?}, line: {}, column: {} }},\n",
                span.kind, span.path, span.line, span.column
            ));
        }
    }
    code.push_str("];\n\n");
    code.push_str(
        "#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]\n\
         pub struct ExampleProvenance {\n\
             pub source_sha256: &'static str,\n\
             pub ir_sha256: &'static str,\n\
         }\n\n",
    );
    code.push_str("pub const EXAMPLES: &[&str] = &[\n");
    for (name, _, _) in &compiled_examples {
        code.push_str(&format!("    {name:?},\n"));
    }
    code.push_str("];\n\n");
    code.push_str("#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]\n");
    code.push_str("pub struct ExampleDefinition {\n");
    code.push_str("    pub name: &'static str,\n");
    code.push_str("    pub source: &'static str,\n");
    code.push_str("}\n\n");
    code.push_str("const DEFINITIONS: &[ExampleDefinition] = &[\n");
    for (name, source, compiled) in &compiled_examples {
        code.push_str(&format!(
            "    ExampleDefinition {{ name: {name:?}, source: {} }},\n",
            rust_string_literal(source)
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

pub fn provenance(name: &str) -> Result<ExampleProvenance> {
    let source_sha256 = SOURCE_SHA256
        .iter()
        .find(|(example, _)| *example == name)
        .map(|(_, hash)| *hash)
        .ok_or_else(|| anyhow::anyhow!("unknown example `{name}`"))?;
    let ir_sha256 = IR_SHA256
        .iter()
        .find(|(example, _)| *example == name)
        .map(|(_, hash)| *hash)
        .ok_or_else(|| anyhow::anyhow!("unknown example `{name}`"))?;
    Ok(ExampleProvenance {
        source_sha256,
        ir_sha256,
    })
}

pub fn app(name: &str) -> Result<ExampleApp> {
    let def = definition(name)?;
    Ok(CompiledApp::new(compile_source(name, def.source)?))
}

"#,
    );
    fs::write(output_path, code)?;
    Ok(())
}

fn rust_string_literal(value: &str) -> String {
    format!("{value:?}")
}
