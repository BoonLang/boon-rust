use anyhow::Result;
use boon_compiler::*;
use boon_shape::Shape;
use boon_source::{SourceEntry, SourceInventory, SourceOwner};
use std::fs;
use std::path::Path;

pub fn generate_manifest(example_name: &str, source_path: &Path, output_path: &Path) -> Result<()> {
    let source = fs::read_to_string(source_path)?;
    let compiled = compile_source(example_name, &source)?;
    let json = serde_json::to_string_pretty(&compiled.sources)?;
    fs::write(output_path, json)?;
    Ok(())
}

pub fn generate_program_metadata(
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

pub fn generate_executable_ir_snapshot(
    example_name: &str,
    source_path: &Path,
    output_path: &Path,
) -> Result<()> {
    let source = fs::read_to_string(source_path)?;
    let compiled = compile_source(example_name, &source)?;
    let json = serde_json::to_string_pretty(&compiled.executable_ir)?;
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
use boon_compiler::*;
use boon_shape::Shape;
use boon_source::{SourceEntry, SourceInventory, SourceOwner};
pub use boon_runtime::{CompiledApp, ExampleApp};

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
        "#[derive(Clone, Debug, Eq, PartialEq)]\n\
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
        "#[derive(Clone, Debug, Eq, PartialEq)]\n\
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
    code.push_str("#[derive(Clone, Debug, Eq, PartialEq)]\n");
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
    match name {
"#,
    );
    for (name, _, compiled) in &compiled_examples {
        code.push_str(&format!(
            "        {name:?} => Ok({}),\n",
            source_inventory_expr(&compiled.sources)
        ));
    }
    code.push_str(
        r#"        _ => Err(anyhow::anyhow!("unknown example `{name}`")),
    }
}

pub fn executable_ir(name: &str) -> Result<ExecutableIr> {
    match name {
"#,
    );
    for (name, _, compiled) in &compiled_examples {
        code.push_str(&format!(
            "        {name:?} => Ok({}),\n",
            executable_ir_expr(&compiled.executable_ir)
        ));
    }
    code.push_str(
        r#"        _ => Err(anyhow::anyhow!("unknown example `{name}`")),
    }
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
    match name {
"#,
    );
    for (name, _, compiled) in &compiled_examples {
        code.push_str(&format!(
            "        {name:?} => Ok(CompiledApp::from_generated_parts({}, {}, {}, {})),\n",
            ir_app_metadata_expr(&compiled.program),
            app_ir_expr(&compiled.app_ir),
            executable_ir_expr(&compiled.executable_ir),
            source_inventory_expr(&compiled.sources)
        ));
    }
    code.push_str(
        r#"        _ => Err(anyhow::anyhow!("unknown example `{name}`")),
    }
}

"#,
    );
    fs::write(output_path, code)?;
    Ok(())
}

fn rust_string_literal(value: &str) -> String {
    format!("{value:?}")
}

fn owned_string_expr(value: &str) -> String {
    format!("{}.to_string()", rust_string_literal(value))
}

fn vec_expr<T>(items: &[T], mut item_expr: impl FnMut(&T) -> String) -> String {
    if items.is_empty() {
        "Vec::new()".to_string()
    } else {
        format!(
            "vec![{}]",
            items
                .iter()
                .map(&mut item_expr)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn option_expr<T>(value: Option<&T>, item_expr: impl FnOnce(&T) -> String) -> String {
    value
        .map(|value| format!("Some({})", item_expr(value)))
        .unwrap_or_else(|| "None".to_string())
}

fn option_string_expr(value: Option<&str>) -> String {
    value
        .map(|value| format!("Some({})", owned_string_expr(value)))
        .unwrap_or_else(|| "None".to_string())
}

fn source_inventory_expr(inventory: &SourceInventory) -> String {
    format!(
        "SourceInventory {{ entries: {} }}",
        vec_expr(&inventory.entries, source_entry_expr)
    )
}

fn source_entry_expr(entry: &SourceEntry) -> String {
    format!(
        "SourceEntry {{ id: {}, path: {}, shape: {}, producer: {}, readers: {}, owner: {} }}",
        entry.id,
        owned_string_expr(&entry.path),
        shape_expr(&entry.shape),
        owned_string_expr(&entry.producer),
        vec_expr(&entry.readers, |reader| owned_string_expr(reader)),
        source_owner_expr(&entry.owner)
    )
}

fn source_owner_expr(owner: &SourceOwner) -> String {
    match owner {
        SourceOwner::Static => "SourceOwner::Static".to_string(),
        SourceOwner::DynamicFamily { owner_path } => format!(
            "SourceOwner::DynamicFamily {{ owner_path: {} }}",
            owned_string_expr(owner_path)
        ),
    }
}

fn shape_expr(shape: &Shape) -> String {
    match shape {
        Shape::Unknown => "Shape::Unknown".to_string(),
        Shape::EmptyRecord => "Shape::EmptyRecord".to_string(),
        Shape::Record(fields) => format!(
            "Shape::Record({})",
            vec_expr(fields, |(key, shape)| format!(
                "({}, {})",
                owned_string_expr(key),
                shape_expr(shape)
            ))
        ),
        Shape::List(item) => format!("Shape::List(Box::new({}))", shape_expr(item)),
        Shape::Text => "Shape::Text".to_string(),
        Shape::Number => "Shape::Number".to_string(),
        Shape::TagSet(tags) => format!(
            "Shape::TagSet({})",
            vec_expr(tags, |tag| owned_string_expr(tag))
        ),
        Shape::Function => "Shape::Function".to_string(),
        Shape::SourceMarker => "Shape::SourceMarker".to_string(),
        Shape::Skip => "Shape::Skip".to_string(),
        Shape::Union(shapes) => format!("Shape::Union({})", vec_expr(shapes, shape_expr)),
    }
}

fn ir_app_metadata_expr(program: &IrAppMetadata) -> String {
    format!(
        "IrAppMetadata {{ title: {}, primary_label: {}, physical_debug: {} }}",
        owned_string_expr(&program.title),
        option_string_expr(program.primary_label.as_deref()),
        program.physical_debug
    )
}

fn executable_ir_expr(ir: &ExecutableIr) -> String {
    format!(
        "ExecutableIr {{ state_slots: {}, source_handlers: {}, scene: {} }}",
        vec_expr(&ir.state_slots, exec_state_slot_expr),
        vec_expr(&ir.source_handlers, exec_source_handler_expr),
        option_expr(ir.scene.as_ref(), ir_render_node_expr)
    )
}

fn exec_state_slot_expr(slot: &ExecStateSlot) -> String {
    format!(
        "ExecStateSlot {{ path: {}, initial: {} }}",
        owned_string_expr(&slot.path),
        exec_expr_expr(&slot.initial)
    )
}

fn exec_source_handler_expr(handler: &ExecSourceHandler) -> String {
    format!(
        "ExecSourceHandler {{ source_path: {}, effects: {} }}",
        owned_string_expr(&handler.source_path),
        vec_expr(&handler.effects, exec_effect_expr)
    )
}

fn exec_effect_expr(effect: &ExecEffect) -> String {
    match effect {
        ExecEffect::SetState { path, value } => format!(
            "ExecEffect::SetState {{ path: {}, value: {} }}",
            owned_string_expr(path),
            exec_expr_expr(value)
        ),
    }
}

fn exec_expr_expr(expr: &ExecExpr) -> String {
    match expr {
        ExecExpr::Number { value } => format!("ExecExpr::Number {{ value: {value} }}"),
        ExecExpr::Text { value } => {
            format!("ExecExpr::Text {{ value: {} }}", owned_string_expr(value))
        }
        ExecExpr::Bool { value } => format!("ExecExpr::Bool {{ value: {value} }}"),
        ExecExpr::Tag { value } => {
            format!("ExecExpr::Tag {{ value: {} }}", owned_string_expr(value))
        }
        ExecExpr::State { path } => {
            format!("ExecExpr::State {{ path: {} }}", owned_string_expr(path))
        }
        ExecExpr::Source { path } => {
            format!("ExecExpr::Source {{ path: {} }}", owned_string_expr(path))
        }
        ExecExpr::Add { left, right } => format!(
            "ExecExpr::Add {{ left: Box::new({}), right: Box::new({}) }}",
            exec_expr_expr(left),
            exec_expr_expr(right)
        ),
        ExecExpr::Subtract { left, right } => format!(
            "ExecExpr::Subtract {{ left: Box::new({}), right: Box::new({}) }}",
            exec_expr_expr(left),
            exec_expr_expr(right)
        ),
        ExecExpr::Equal { left, right } => format!(
            "ExecExpr::Equal {{ left: Box::new({}), right: Box::new({}) }}",
            exec_expr_expr(left),
            exec_expr_expr(right)
        ),
        ExecExpr::TextFromNumber { value } => format!(
            "ExecExpr::TextFromNumber {{ value: Box::new({}) }}",
            exec_expr_expr(value)
        ),
        ExecExpr::Call { path, args } => format!(
            "ExecExpr::Call {{ path: {}, args: {} }}",
            owned_string_expr(path),
            vec_expr(args, exec_call_arg_expr)
        ),
        ExecExpr::When { input, arms } => format!(
            "ExecExpr::When {{ input: {}, arms: {} }}",
            option_expr(input.as_deref(), |expr| format!(
                "Box::new({})",
                exec_expr_expr(expr)
            )),
            vec_expr(arms, exec_when_arm_expr)
        ),
        ExecExpr::Skip => "ExecExpr::Skip".to_string(),
    }
}

fn exec_call_arg_expr(arg: &ExecCallArg) -> String {
    format!(
        "ExecCallArg {{ name: {}, value: {} }}",
        owned_string_expr(&arg.name),
        exec_expr_expr(&arg.value)
    )
}

fn exec_when_arm_expr(arm: &ExecWhenArm) -> String {
    format!(
        "ExecWhenArm {{ pattern: {}, value: {} }}",
        owned_string_expr(&arm.pattern),
        exec_expr_expr(&arm.value)
    )
}

fn app_ir_expr(ir: &AppIr) -> String {
    format!(
        "AppIr {{ state_cells: {}, derived_values: {}, expression_surface: {}, collection_states: {}, static_records: {}, event_handlers: {}, render_tree: {} }}",
        vec_expr(&ir.state_cells, ir_state_cell_expr),
        vec_expr(&ir.derived_values, ir_derived_value_expr),
        option_expr(ir.expression_surface.as_ref(), ir_expression_surface_expr),
        vec_expr(&ir.collection_states, ir_collection_state_expr),
        vec_expr(&ir.static_records, ir_static_record_expr),
        vec_expr(&ir.event_handlers, ir_event_handler_expr),
        option_expr(ir.render_tree.as_ref(), ir_render_node_expr)
    )
}

fn ir_state_cell_expr(cell: &IrStateCell) -> String {
    format!(
        "IrStateCell {{ path: {}, initial: {} }}",
        owned_string_expr(&cell.path),
        ir_value_expr_expr(&cell.initial)
    )
}

fn ir_derived_value_expr(value: &IrDerivedValue) -> String {
    format!(
        "IrDerivedValue {{ path: {}, expr: {} }}",
        owned_string_expr(&value.path),
        ir_derived_expr_expr(&value.expr)
    )
}

fn ir_derived_expr_expr(expr: &IrDerivedExpr) -> String {
    match expr {
        IrDerivedExpr::CollectionCount { collection_path } => format!(
            "IrDerivedExpr::CollectionCount {{ collection_path: {} }}",
            owned_string_expr(collection_path)
        ),
        IrDerivedExpr::CollectionCountWhere {
            collection_path,
            predicate,
        } => format!(
            "IrDerivedExpr::CollectionCountWhere {{ collection_path: {}, predicate: {} }}",
            owned_string_expr(collection_path),
            ir_collection_predicate_expr(predicate)
        ),
        IrDerivedExpr::Subtract { left, right } => format!(
            "IrDerivedExpr::Subtract {{ left: {}, right: {} }}",
            owned_string_expr(left),
            owned_string_expr(right)
        ),
        IrDerivedExpr::Equal { left, right } => format!(
            "IrDerivedExpr::Equal {{ left: {}, right: {} }}",
            owned_string_expr(left),
            owned_string_expr(right)
        ),
    }
}

fn ir_expression_surface_expr(surface: &IrExpressionSurface) -> String {
    format!(
        "IrExpressionSurface {{ root: {}, rows: {}, columns: {}, functions: {} }}",
        owned_string_expr(&surface.root),
        surface.rows,
        surface.columns,
        vec_expr(&surface.functions, |function| owned_string_expr(function))
    )
}

fn ir_collection_state_expr(state: &IrCollectionState) -> String {
    format!(
        "IrCollectionState {{ path: {}, initial_entries: {} }}",
        owned_string_expr(&state.path),
        vec_expr(&state.initial_entries, ir_collection_seed_expr)
    )
}

fn ir_collection_seed_expr(seed: &IrCollectionSeed) -> String {
    format!(
        "IrCollectionSeed {{ fields: {} }}",
        vec_expr(&seed.fields, ir_literal_field_expr)
    )
}

fn ir_static_record_expr(record: &IrStaticRecord) -> String {
    format!(
        "IrStaticRecord {{ path: {}, fields: {} }}",
        owned_string_expr(&record.path),
        vec_expr(&record.fields, ir_literal_field_expr)
    )
}

fn ir_literal_field_expr(field: &IrStaticField) -> String {
    format!(
        "IrStaticField {{ key: {}, value: {} }}",
        owned_string_expr(&field.key),
        ir_static_value_expr(&field.value)
    )
}

fn ir_static_value_expr(value: &IrStaticValue) -> String {
    match value {
        IrStaticValue::Text { value } => format!(
            "IrStaticValue::Text {{ value: {} }}",
            owned_string_expr(value)
        ),
        IrStaticValue::Number { value } => {
            format!("IrStaticValue::Number {{ value: {value} }}")
        }
        IrStaticValue::Bool { value } => format!("IrStaticValue::Bool {{ value: {value} }}"),
        IrStaticValue::Tag { value } => {
            format!(
                "IrStaticValue::Tag {{ value: {} }}",
                owned_string_expr(value)
            )
        }
        IrStaticValue::Path { value } => {
            format!(
                "IrStaticValue::Path {{ value: {} }}",
                owned_string_expr(value)
            )
        }
        IrStaticValue::Range { from, to } => {
            format!("IrStaticValue::Range {{ from: {from}, to: {to} }}")
        }
        IrStaticValue::Record { fields } => format!(
            "IrStaticValue::Record {{ fields: {} }}",
            vec_expr(fields, ir_literal_field_expr)
        ),
        IrStaticValue::List { items } => format!(
            "IrStaticValue::List {{ items: {} }}",
            vec_expr(items, ir_static_value_expr)
        ),
    }
}

fn ir_event_handler_expr(handler: &IrEventHandler) -> String {
    format!(
        "IrEventHandler {{ source_path: {}, when: {}, effects: {} }}",
        owned_string_expr(&handler.source_path),
        option_expr(handler.when.as_ref(), ir_predicate_expr),
        vec_expr(&handler.effects, ir_effect_expr)
    )
}

fn ir_effect_expr(effect: &IrEffect) -> String {
    match effect {
        IrEffect::Assign { state_path, expr } => format!(
            "IrEffect::Assign {{ state_path: {}, expr: {} }}",
            owned_string_expr(state_path),
            ir_value_expr_expr(expr)
        ),
        IrEffect::CollectionAppendRecord {
            collection_path,
            fields,
            skip_if_empty_field,
        } => format!(
            "IrEffect::CollectionAppendRecord {{ collection_path: {}, fields: {}, skip_if_empty_field: {} }}",
            owned_string_expr(collection_path),
            vec_expr(fields, ir_collection_field_assignment_expr),
            option_string_expr(skip_if_empty_field.as_deref())
        ),
        IrEffect::CollectionUpdateAllFields {
            collection_path,
            field,
            value,
        } => format!(
            "IrEffect::CollectionUpdateAllFields {{ collection_path: {}, field: {}, value: {} }}",
            owned_string_expr(collection_path),
            owned_string_expr(field),
            ir_collection_value_expr_expr(value)
        ),
        IrEffect::CollectionUpdateOwnerField {
            collection_path,
            field,
            value,
        } => format!(
            "IrEffect::CollectionUpdateOwnerField {{ collection_path: {}, field: {}, value: {} }}",
            owned_string_expr(collection_path),
            owned_string_expr(field),
            ir_collection_value_expr_expr(value)
        ),
        IrEffect::CollectionRemoveCurrent { collection_path } => format!(
            "IrEffect::CollectionRemoveCurrent {{ collection_path: {} }}",
            owned_string_expr(collection_path)
        ),
        IrEffect::CollectionRemoveWhere {
            collection_path,
            predicate,
        } => format!(
            "IrEffect::CollectionRemoveWhere {{ collection_path: {}, predicate: {} }}",
            owned_string_expr(collection_path),
            ir_collection_predicate_expr(predicate)
        ),
        IrEffect::SetTagState { state_path, value } => format!(
            "IrEffect::SetTagState {{ state_path: {}, value: {} }}",
            owned_string_expr(state_path),
            owned_string_expr(value)
        ),
        IrEffect::ClearText { text_state_path } => format!(
            "IrEffect::ClearText {{ text_state_path: {} }}",
            owned_string_expr(text_state_path)
        ),
    }
}

fn ir_collection_field_assignment_expr(field: &IrCollectionFieldAssignment) -> String {
    format!(
        "IrCollectionFieldAssignment {{ field: {}, value: {} }}",
        owned_string_expr(&field.field),
        ir_collection_value_expr_expr(&field.value)
    )
}

fn ir_collection_value_expr_expr(value: &IrCollectionValueExpr) -> String {
    match value {
        IrCollectionValueExpr::Static { value } => format!(
            "IrCollectionValueExpr::Static {{ value: {} }}",
            ir_static_value_expr(value)
        ),
        IrCollectionValueExpr::SourceText { path, trim } => format!(
            "IrCollectionValueExpr::SourceText {{ path: {}, trim: {} }}",
            owned_string_expr(path),
            trim
        ),
        IrCollectionValueExpr::NotOwnerBoolField { field } => format!(
            "IrCollectionValueExpr::NotOwnerBoolField {{ field: {} }}",
            owned_string_expr(field)
        ),
        IrCollectionValueExpr::NotAllBoolField { field } => format!(
            "IrCollectionValueExpr::NotAllBoolField {{ field: {} }}",
            owned_string_expr(field)
        ),
    }
}

fn ir_collection_predicate_expr(predicate: &IrCollectionPredicate) -> String {
    match predicate {
        IrCollectionPredicate::FieldBoolEquals { field, value } => format!(
            "IrCollectionPredicate::FieldBoolEquals {{ field: {}, value: {} }}",
            owned_string_expr(field),
            value
        ),
    }
}

fn ir_predicate_expr(predicate: &IrPredicate) -> String {
    match predicate {
        IrPredicate::SourceTagEquals { path, tag } => format!(
            "IrPredicate::SourceTagEquals {{ path: {}, tag: {} }}",
            owned_string_expr(path),
            owned_string_expr(tag)
        ),
    }
}

fn ir_value_expr_expr(expr: &IrValueExpr) -> String {
    match expr {
        IrValueExpr::Number { value } => format!("IrValueExpr::Number {{ value: {value} }}"),
        IrValueExpr::Hold { state_path } => format!(
            "IrValueExpr::Hold {{ state_path: {} }}",
            owned_string_expr(state_path)
        ),
        IrValueExpr::Add { left, right } => format!(
            "IrValueExpr::Add {{ left: Box::new({}), right: Box::new({}) }}",
            ir_value_expr_expr(left),
            ir_value_expr_expr(right)
        ),
        IrValueExpr::Source { path } => format!(
            "IrValueExpr::Source {{ path: {} }}",
            owned_string_expr(path)
        ),
        IrValueExpr::Skip => "IrValueExpr::Skip".to_string(),
    }
}

fn ir_render_node_expr(node: &IrRenderNode) -> String {
    format!(
        "IrRenderNode {{ id: {}, kind: {}, source_path: {}, collection_path: {}, text: {}, bounds: {}, scale: {}, color: {}, children: {} }}",
        owned_string_expr(&node.id),
        ir_render_kind_expr(&node.kind),
        option_string_expr(node.source_path.as_deref()),
        option_string_expr(node.collection_path.as_deref()),
        option_expr(node.text.as_ref(), ir_render_text_expr),
        option_expr(node.bounds.as_ref(), ir_render_bounds_expr),
        option_expr(node.scale.as_ref(), ir_render_number_expr),
        node.color
            .map(|color| format!(
                "Some([{}, {}, {}, {}])",
                color[0], color[1], color[2], color[3]
            ))
            .unwrap_or_else(|| "None".to_string()),
        vec_expr(&node.children, ir_render_node_expr)
    )
}

fn ir_render_kind_expr(kind: &IrRenderKind) -> String {
    match kind {
        IrRenderKind::Root => "IrRenderKind::Root",
        IrRenderKind::Panel => "IrRenderKind::Panel",
        IrRenderKind::Text => "IrRenderKind::Text",
        IrRenderKind::Button => "IrRenderKind::Button",
        IrRenderKind::TextInput => "IrRenderKind::TextInput",
        IrRenderKind::Checkbox => "IrRenderKind::Checkbox",
        IrRenderKind::Label => "IrRenderKind::Label",
        IrRenderKind::Grid => "IrRenderKind::Grid",
        IrRenderKind::Rect => "IrRenderKind::Rect",
        IrRenderKind::ListMap => "IrRenderKind::ListMap",
        IrRenderKind::Unknown => "IrRenderKind::Unknown",
    }
    .to_string()
}

fn ir_render_bounds_expr(bounds: &IrRenderBounds) -> String {
    format!(
        "IrRenderBounds {{ x: {}, y: {}, width: {}, height: {} }}",
        ir_render_number_expr(&bounds.x),
        ir_render_number_expr(&bounds.y),
        ir_render_number_expr(&bounds.width),
        ir_render_number_expr(&bounds.height)
    )
}

fn ir_render_number_expr(number: &IrRenderNumber) -> String {
    match number {
        IrRenderNumber::Literal { value } => {
            format!("IrRenderNumber::Literal {{ value: {value} }}")
        }
        IrRenderNumber::Binding { path } => format!(
            "IrRenderNumber::Binding {{ path: {} }}",
            owned_string_expr(path)
        ),
    }
}

fn ir_render_text_expr(text: &IrRenderText) -> String {
    match text {
        IrRenderText::Literal { value } => format!(
            "IrRenderText::Literal {{ value: {} }}",
            owned_string_expr(value)
        ),
        IrRenderText::Binding { path } => format!(
            "IrRenderText::Binding {{ path: {} }}",
            owned_string_expr(path)
        ),
    }
}
