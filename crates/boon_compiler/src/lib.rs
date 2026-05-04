use anyhow::{Result, bail};
use boon_hir::{
    HirCallArg, HirExpr, HirExprKind, HirItem, HirListOp, HirLiteral, HirModule, HirRecord, lower,
};
use boon_host_schema::{HostContract, element_contracts};
use boon_shape::Shape;
use boon_source::{SourceEntry, SourceInventory, SourceOwner};
use boon_syntax::{
    AstExpr, AstExprKind, AstItem, AstRecord, ParsedModule, ParsedRecordEntry, parse_module,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompiledModule {
    pub name: String,
    pub hir: HirModule,
    pub sources: SourceInventory,
    pub program: IrAppMetadata,
    pub app_ir: AppIr,
    pub executable_ir: ExecutableIr,
    pub provenance: CompiledProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompiledProvenance {
    pub source_sha256: String,
    pub hir_sha256: String,
    pub source_spans: Vec<CompiledSourceSpan>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompiledSourceSpan {
    pub kind: String,
    pub path: String,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AppIr {
    pub state_cells: Vec<IrStateCell>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_values: Vec<IrDerivedValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression_surface: Option<IrExpressionSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collection_states: Vec<IrCollectionState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub static_records: Vec<IrStaticRecord>,
    pub event_handlers: Vec<IrEventHandler>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_tree: Option<IrRenderNode>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutableIr {
    pub state_slots: Vec<ExecStateSlot>,
    pub source_handlers: Vec<ExecSourceHandler>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<IrRenderNode>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecStateSlot {
    pub path: String,
    pub initial: ExecExpr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecSourceHandler {
    pub source_path: String,
    pub effects: Vec<ExecEffect>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecCallArg {
    pub name: String,
    pub value: ExecExpr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecWhenArm {
    pub pattern: String,
    pub value: ExecExpr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecEffect {
    SetState { path: String, value: ExecExpr },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecExpr {
    Number {
        value: i64,
    },
    Text {
        value: String,
    },
    Bool {
        value: bool,
    },
    Tag {
        value: String,
    },
    State {
        path: String,
    },
    Source {
        path: String,
    },
    Add {
        left: Box<ExecExpr>,
        right: Box<ExecExpr>,
    },
    Subtract {
        left: Box<ExecExpr>,
        right: Box<ExecExpr>,
    },
    Equal {
        left: Box<ExecExpr>,
        right: Box<ExecExpr>,
    },
    TextFromNumber {
        value: Box<ExecExpr>,
    },
    Call {
        path: String,
        args: Vec<ExecCallArg>,
    },
    When {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Box<ExecExpr>>,
        arms: Vec<ExecWhenArm>,
    },
    Skip,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrStateCell {
    pub path: String,
    pub initial: IrValueExpr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrDerivedValue {
    pub path: String,
    pub expr: IrDerivedExpr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrExpressionSurface {
    pub root: String,
    pub rows: usize,
    pub columns: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrDerivedExpr {
    CollectionCount {
        collection_path: String,
    },
    CollectionCountWhere {
        collection_path: String,
        predicate: IrCollectionPredicate,
    },
    Subtract {
        left: String,
        right: String,
    },
    Equal {
        left: String,
        right: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrCollectionState {
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initial_entries: Vec<IrCollectionSeed>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrCollectionSeed {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<IrStaticField>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrStaticRecord {
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<IrStaticField>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrStaticField {
    pub key: String,
    pub value: IrStaticValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrStaticValue {
    Text { value: String },
    Number { value: i64 },
    Bool { value: bool },
    Tag { value: String },
    Path { value: String },
    Range { from: i64, to: i64 },
    Record { fields: Vec<IrStaticField> },
    List { items: Vec<IrStaticValue> },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrEventHandler {
    pub source_path: String,
    pub when: Option<IrPredicate>,
    pub effects: Vec<IrEffect>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrRenderNode {
    pub id: String,
    pub kind: IrRenderKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<IrRenderText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<IrRenderBounds>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<IrRenderNumber>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 4]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<IrRenderNode>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IrRenderKind {
    Root,
    Panel,
    Text,
    Button,
    TextInput,
    Checkbox,
    Label,
    Grid,
    Rect,
    ListMap,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrRenderBounds {
    pub x: IrRenderNumber,
    pub y: IrRenderNumber,
    pub width: IrRenderNumber,
    pub height: IrRenderNumber,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrRenderNumber {
    Literal { value: i64 },
    Binding { path: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrRenderText {
    Literal { value: String },
    Binding { path: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrEffect {
    Assign {
        state_path: String,
        expr: IrValueExpr,
    },
    CollectionAppendRecord {
        collection_path: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fields: Vec<IrCollectionFieldAssignment>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skip_if_empty_field: Option<String>,
    },
    CollectionUpdateAllFields {
        collection_path: String,
        field: String,
        value: IrCollectionValueExpr,
    },
    CollectionUpdateOwnerField {
        collection_path: String,
        field: String,
        value: IrCollectionValueExpr,
    },
    CollectionRemoveCurrent {
        collection_path: String,
    },
    CollectionRemoveWhere {
        collection_path: String,
        predicate: IrCollectionPredicate,
    },
    SetTagState {
        state_path: String,
        value: String,
    },
    ClearText {
        text_state_path: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrCollectionFieldAssignment {
    pub field: String,
    pub value: IrCollectionValueExpr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrCollectionValueExpr {
    Static { value: IrStaticValue },
    SourceText { path: String, trim: bool },
    NotOwnerBoolField { field: String },
    NotAllBoolField { field: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrCollectionPredicate {
    FieldBoolEquals { field: String, value: bool },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrPredicate {
    SourceTagEquals { path: String, tag: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrValueExpr {
    Number {
        value: i64,
    },
    Hold {
        state_path: String,
    },
    Add {
        left: Box<IrValueExpr>,
        right: Box<IrValueExpr>,
    },
    Source {
        path: String,
    },
    Skip,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrAppMetadata {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_label: Option<String>,
    pub physical_debug: bool,
}

pub fn compile_source(name: &str, source: &str) -> Result<CompiledModule> {
    let parsed = parse_module(name, source)?;
    let hir = lower(parsed.clone());
    if !hir.diagnostics.is_empty() {
        let diagnostics = hir
            .diagnostics
            .iter()
            .map(|diagnostic| {
                format!(
                    "line {} column {}: {}",
                    diagnostic.span.line, diagnostic.span.column, diagnostic.message
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        bail!("unsupported Boon syntax in module `{name}`: {diagnostics}");
    }
    validate_supported_module_paths(&parsed)?;
    let provenance = compiled_provenance(source, &hir)?;
    let dynamic_sequence_root = infer_dynamic_sequence_root(&parsed);
    let dynamic_mapped_root = infer_dynamic_mapped_source_root(&parsed);
    let map_alias_roots = map_alias_roots(
        &parsed,
        dynamic_sequence_root.as_deref(),
        dynamic_mapped_root.as_deref(),
    );
    let host_bindings = collect_host_bindings(&parsed, &map_alias_roots);
    let contracts = element_contracts();
    let mut seen = HashSet::new();
    let mut entries = Vec::new();

    for leaf in &parsed.source_leaves {
        let source_path = normalize_source_path(&leaf.path, dynamic_sequence_root.as_deref());
        if source_path.is_empty() {
            bail!("SOURCE at line {} has no data path", leaf.span.line);
        }
        if !seen.insert(source_path.clone()) {
            bail!("SOURCE path `{}` is declared more than once", source_path);
        }
        let binding = binding_for_source(&source_path, &host_bindings, &contracts)?;
        let shape = binding.shape;
        let owner = if source_path.contains("[*]") {
            SourceOwner::DynamicFamily {
                owner_path: owner_path(&source_path),
            }
        } else {
            SourceOwner::Static
        };
        entries.push(SourceEntry {
            id: entries.len(),
            path: source_path,
            shape,
            producer: format!("{}(element.{})", binding.function, binding.relative_path),
            readers: vec!["compiled logic".to_string()],
            owner,
        });
    }

    if !parsed
        .module_calls
        .iter()
        .any(|call| call.path.starts_with("Element/"))
    {
        bail!("module `{name}` has SOURCE leaves but no Element/* host bindings");
    }

    let sources = SourceInventory { entries };
    validate_static_source_reads(&hir, &sources)?;
    let app_ir = app_ir_from_hir(&hir, &sources);
    let executable_ir = executable_ir_from_hir(&hir, &sources);
    let program = app_metadata(name, &parsed);
    Ok(CompiledModule {
        name: name.to_string(),
        hir,
        sources,
        program,
        app_ir,
        executable_ir,
        provenance,
    })
}

fn validate_supported_module_paths(parsed: &ParsedModule) -> Result<()> {
    for call in &parsed.module_calls {
        match call.path.as_str() {
            "Document/new"
            | "Element/panel"
            | "Element/text"
            | "Element/rect"
            | "Element/button"
            | "Element/text_input"
            | "Element/checkbox"
            | "Element/label"
            | "Element/grid"
            | "List/append"
            | "List/remove"
            | "List/retain"
            | "List/map"
            | "List/count"
            | "List/range"
            | "Text/from_number"
            | "Text/trim"
            | "Text/is_not_empty"
            | "Math/add"
            | "Math/sum"
            | "Number/min"
            | "Number/max"
            | "Number/clamp"
            | "Number/abs"
            | "Number/neg_abs"
            | "Number/scale_percent"
            | "Number/percent_of_range"
            | "Number/less_than"
            | "Number/less_or_equal"
            | "Number/greater_than"
            | "Number/greater_or_equal"
            | "Geometry/intersects" => {}
            path if path.starts_with("Geometry/") => bail!(
                "unsupported Geometry operation `{}` at line {} column {}",
                path,
                call.span.line,
                call.span.column
            ),
            path if path.starts_with("Number/") => bail!(
                "unsupported Number operation `{}` at line {} column {}",
                path,
                call.span.line,
                call.span.column
            ),
            path if path.starts_with("Math/") => bail!(
                "unsupported Math operation `{}` at line {} column {}",
                path,
                call.span.line,
                call.span.column
            ),
            path if path.starts_with("List/") => bail!(
                "unsupported List operation `{}` at line {} column {}",
                path,
                call.span.line,
                call.span.column
            ),
            path if path.starts_with("Element/")
                || path.starts_with("Document/")
                || path.starts_with("Text/") =>
            {
                bail!(
                    "unsupported Boon module call `{}` at line {} column {}",
                    path,
                    call.span.line,
                    call.span.column
                )
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_static_source_reads(hir: &HirModule, sources: &SourceInventory) -> Result<()> {
    let mut reads = Vec::new();
    for item in &hir.items {
        collect_static_source_reads_from_item(item, &mut reads);
    }
    reads.sort();
    reads.dedup();
    for path in reads {
        if !source_path_backed(sources, &path) {
            bail!("source path `{path}` is read but no host/runtime producer is bound");
        }
    }
    Ok(())
}

fn collect_static_source_reads_from_item(item: &HirItem, reads: &mut Vec<String>) {
    match item {
        HirItem::Record(record) => collect_static_source_reads_from_record(record, reads),
        HirItem::Function(function) => collect_static_source_reads_from_expr(&function.body, reads),
        HirItem::Expression(expr) => collect_static_source_reads_from_expr(expr, reads),
    }
}

fn collect_static_source_reads_from_record(record: &HirRecord, reads: &mut Vec<String>) {
    if let Some(value) = &record.value {
        collect_static_source_reads_from_expr(value, reads);
    }
    for child in &record.children {
        collect_static_source_reads_from_record(child, reads);
    }
}

fn collect_static_source_reads_from_expr(expr: &HirExpr, reads: &mut Vec<String>) {
    match &expr.kind {
        HirExprKind::Path { value } if value.starts_with("store.sources.") => {
            reads.push(value.clone());
        }
        HirExprKind::Record { entries } => {
            for entry in entries {
                collect_static_source_reads_from_record(entry, reads);
            }
        }
        HirExprKind::List { items } | HirExprKind::Latest { branches: items } => {
            for item in items {
                collect_static_source_reads_from_expr(item, reads);
            }
        }
        HirExprKind::Block { bindings } => {
            for binding in bindings {
                collect_static_source_reads_from_expr(&binding.value, reads);
            }
        }
        HirExprKind::When { arms } | HirExprKind::While { arms } => {
            for arm in arms {
                collect_static_source_reads_from_expr(&arm.value, reads);
            }
        }
        HirExprKind::Then { body } | HirExprKind::Hold { body, .. } => {
            collect_static_source_reads_from_expr(body, reads);
        }
        HirExprKind::HostCall { args, .. }
        | HirExprKind::ListCall { args, .. }
        | HirExprKind::FunctionCall { args, .. } => {
            for arg in args {
                collect_static_source_reads_from_expr(&arg.value, reads);
            }
        }
        HirExprKind::Pipeline { input, stages } => {
            collect_static_source_reads_from_expr(input, reads);
            for stage in stages {
                collect_static_source_reads_from_expr(stage, reads);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_static_source_reads_from_expr(left, reads);
            collect_static_source_reads_from_expr(right, reads);
        }
        _ => {}
    }
}

fn source_path_backed(sources: &SourceInventory, path: &str) -> bool {
    sources
        .entries
        .iter()
        .any(|entry| entry.path == path || entry.path.starts_with(&format!("{path}.")))
}

fn app_ir_from_hir(hir: &HirModule, sources: &SourceInventory) -> AppIr {
    let mut ir = AppIr::default();
    let collection_paths = hir
        .items
        .iter()
        .filter_map(|item| match item {
            HirItem::Record(record) if record_value_is_list(record) => Some(record.key.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let primary_collection_path = collection_paths.first().cloned().unwrap_or_else(|| {
        dynamic_source_families(sources).first().map_or_else(
            || "records".to_string(),
            |family| dynamic_family_root(family),
        )
    });
    ir.expression_surface = expression_surface_from_hir(hir, sources);

    for item in &hir.items {
        let HirItem::Record(record) = item else {
            continue;
        };
        if let Some((event_path, initial, step)) = accumulator_from_hir_record(record, sources) {
            push_accumulator_ir(&mut ir, &event_path, &record.key, initial, step);
        }
        push_derived_value_from_hir_record(&mut ir, record);
        push_collection_state_from_hir_record(&mut ir, hir, record);
        push_static_record_from_hir_record(&mut ir, record);
        push_collection_handlers_from_hir_record(&mut ir, hir, sources, record);
        push_selector_handlers_from_hir_record(&mut ir, sources, record);
    }
    push_item_state_handlers_from_hir(&mut ir, sources, &primary_collection_path, hir);
    ir.render_tree = render_tree_from_hir(hir, sources);
    dedupe_app_ir(&mut ir);
    ir
}

fn executable_ir_from_hir(hir: &HirModule, sources: &SourceInventory) -> ExecutableIr {
    let mut executable = ExecutableIr {
        scene: render_tree_from_hir(hir, sources),
        ..ExecutableIr::default()
    };
    for item in &hir.items {
        let HirItem::Record(record) = item else {
            continue;
        };
        push_executable_hold_handlers(&mut executable, record, None, sources);
    }
    executable
}

fn push_executable_hold_handlers(
    executable: &mut ExecutableIr,
    record: &HirRecord,
    parent_path: Option<&str>,
    sources: &SourceInventory,
) {
    let state_path = parent_path.map_or_else(
        || record.key.clone(),
        |parent| format!("{parent}.{}", record.key),
    );
    if let Some((source_path, initial, value)) =
        executable_hold_handler_from_record(record, &state_path, sources)
    {
        executable.state_slots.push(ExecStateSlot {
            path: state_path.clone(),
            initial,
        });
        executable.source_handlers.push(ExecSourceHandler {
            source_path,
            effects: vec![ExecEffect::SetState {
                path: state_path.clone(),
                value,
            }],
        });
    }
    for child in &record.children {
        push_executable_hold_handlers(executable, child, Some(&state_path), sources);
    }
}

fn executable_hold_handler_from_record(
    record: &HirRecord,
    state_path: &str,
    sources: &SourceInventory,
) -> Option<(String, ExecExpr, ExecExpr)> {
    let expr = record.value.as_ref()?;
    let (input, stages) = pipeline_parts(expr)?;
    let initial = exec_expr_from_hir(input, None, sources)?;
    if !matches!(initial, ExecExpr::Number { .. }) {
        return None;
    }
    let (state_name, hold_body) = stages.iter().find_map(hold_stage)?;
    let (source_path, value) =
        executable_source_then_expr(hold_body, state_name, state_path, sources)?;
    Some((source_path, initial, value))
}

fn executable_source_then_expr(
    expr: &HirExpr,
    state_name: &str,
    state_path: &str,
    sources: &SourceInventory,
) -> Option<(String, ExecExpr)> {
    match &expr.kind {
        HirExprKind::Pipeline { input, stages } => {
            let source_path = resolve_source_path(sources, path_expr(input)?)?;
            let then = stages.iter().find_map(then_body)?;
            let value = exec_expr_from_hir(then, Some((state_name, state_path)), sources)?;
            Some((source_path, value))
        }
        HirExprKind::Latest { branches } => branches.iter().find_map(|branch| {
            executable_source_then_expr(branch, state_name, state_path, sources)
        }),
        _ => None,
    }
}

fn exec_expr_from_hir(
    expr: &HirExpr,
    hold_state: Option<(&str, &str)>,
    sources: &SourceInventory,
) -> Option<ExecExpr> {
    match &expr.kind {
        HirExprKind::Literal {
            literal: HirLiteral::Number { value },
        } => Some(ExecExpr::Number { value: *value }),
        HirExprKind::Literal {
            literal: HirLiteral::Text { value },
        } => Some(ExecExpr::Text {
            value: value.clone(),
        }),
        HirExprKind::Literal {
            literal: HirLiteral::Bool { value },
        } => Some(ExecExpr::Bool { value: *value }),
        HirExprKind::Tag { value } => Some(ExecExpr::Tag {
            value: value.clone(),
        }),
        HirExprKind::Skip => Some(ExecExpr::Skip),
        HirExprKind::Path { value } => {
            if let Some((state_name, state_path)) = hold_state
                && value == state_name
            {
                return Some(ExecExpr::State {
                    path: state_path.to_string(),
                });
            }
            if let Some(path) = resolve_source_path(sources, value) {
                return Some(ExecExpr::Source { path });
            }
            Some(ExecExpr::State {
                path: value.clone(),
            })
        }
        HirExprKind::Binary { op, left, right } => {
            let left = Box::new(exec_expr_from_hir(left, hold_state, sources)?);
            let right = Box::new(exec_expr_from_hir(right, hold_state, sources)?);
            match op {
                boon_syntax::AstBinaryOp::Add => Some(ExecExpr::Add { left, right }),
                boon_syntax::AstBinaryOp::Subtract => Some(ExecExpr::Subtract { left, right }),
                boon_syntax::AstBinaryOp::Equal => Some(ExecExpr::Equal { left, right }),
            }
        }
        HirExprKind::FunctionCall { path, args } => Some(ExecExpr::Call {
            path: path.clone(),
            args: args
                .iter()
                .map(|arg| {
                    Some(ExecCallArg {
                        name: arg.name.clone(),
                        value: exec_expr_from_hir(&arg.value, hold_state, sources)?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        }),
        HirExprKind::When { arms } => Some(ExecExpr::When {
            input: None,
            arms: arms
                .iter()
                .map(|arm| {
                    Some(ExecWhenArm {
                        pattern: arm.pattern.clone(),
                        value: exec_expr_from_hir(&arm.value, hold_state, sources)?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        }),
        HirExprKind::Pipeline { input, stages } => {
            if stages.len() == 1
                && let HirExprKind::FunctionCall { path, .. } = &stages[0].kind
                && path == "Text/from_number"
            {
                return Some(ExecExpr::TextFromNumber {
                    value: Box::new(exec_expr_from_hir(input, hold_state, sources)?),
                });
            }
            if stages.len() == 1
                && let HirExprKind::When { arms } = &stages[0].kind
            {
                return Some(ExecExpr::When {
                    input: Some(Box::new(exec_expr_from_hir(input, hold_state, sources)?)),
                    arms: arms
                        .iter()
                        .map(|arm| {
                            Some(ExecWhenArm {
                                pattern: arm.pattern.clone(),
                                value: exec_expr_from_hir(&arm.value, hold_state, sources)?,
                            })
                        })
                        .collect::<Option<Vec<_>>>()?,
                });
            }
            None
        }
        _ => None,
    }
}

fn render_tree_from_hir(hir: &HirModule, sources: &SourceInventory) -> Option<IrRenderNode> {
    let document = hir_record(hir, "document")?.value.as_ref()?;
    let mut ids = RenderIdAllocator::default();
    let root = document_root_expr(document)?;
    Some(IrRenderNode {
        id: ids.next("root"),
        kind: IrRenderKind::Root,
        source_path: None,
        collection_path: None,
        text: None,
        bounds: None,
        scale: None,
        color: None,
        children: vec![render_node_from_expr(root, sources, &mut ids)],
    })
}

fn document_root_expr(expr: &HirExpr) -> Option<&HirExpr> {
    match &expr.kind {
        HirExprKind::HostCall { path, args } if path == "Document/new" => named_arg(args, "root"),
        _ => None,
    }
}

#[derive(Default)]
struct RenderIdAllocator {
    next: u64,
}

impl RenderIdAllocator {
    fn next(&mut self, prefix: &str) -> String {
        let id = self.next;
        self.next += 1;
        format!("{prefix}_{id}")
    }
}

fn render_node_from_expr(
    expr: &HirExpr,
    sources: &SourceInventory,
    ids: &mut RenderIdAllocator,
) -> IrRenderNode {
    match &expr.kind {
        HirExprKind::HostCall { path, args } => render_host_node(path, args, sources, ids),
        HirExprKind::Pipeline { input, stages } => {
            render_pipeline_node(input, stages, sources, ids)
        }
        HirExprKind::List { items } => IrRenderNode {
            id: ids.next("list"),
            kind: IrRenderKind::Panel,
            source_path: None,
            collection_path: None,
            text: None,
            bounds: None,
            scale: None,
            color: None,
            children: items
                .iter()
                .map(|item| render_node_from_expr(item, sources, ids))
                .collect(),
        },
        _ => IrRenderNode {
            id: ids.next("unknown"),
            kind: IrRenderKind::Unknown,
            source_path: None,
            collection_path: None,
            text: render_text_from_expr(expr),
            bounds: None,
            scale: None,
            color: None,
            children: Vec::new(),
        },
    }
}

fn render_host_node(
    path: &str,
    args: &[HirCallArg],
    sources: &SourceInventory,
    ids: &mut RenderIdAllocator,
) -> IrRenderNode {
    let kind = match path {
        "Element/panel" => IrRenderKind::Panel,
        "Element/text" => IrRenderKind::Text,
        "Element/rect" => IrRenderKind::Rect,
        "Element/button" => IrRenderKind::Button,
        "Element/text_input" => IrRenderKind::TextInput,
        "Element/checkbox" => IrRenderKind::Checkbox,
        "Element/label" => IrRenderKind::Label,
        "Element/grid" => IrRenderKind::Grid,
        _ => IrRenderKind::Unknown,
    };
    let source_path = named_arg(args, "element")
        .and_then(path_expr)
        .and_then(|path| resolve_source_base(sources, path));
    let text = named_arg(args, "label")
        .or_else(|| named_arg(args, "text"))
        .and_then(render_text_from_expr);
    let children = named_arg(args, "children")
        .or_else(|| named_arg(args, "cells"))
        .map(|children| render_children_from_expr(children, sources, ids))
        .unwrap_or_default();
    IrRenderNode {
        id: ids.next(path.rsplit('/').next().unwrap_or("node")),
        kind,
        source_path,
        collection_path: None,
        text,
        bounds: render_bounds_from_args(args),
        scale: named_arg(args, "scale").and_then(render_number_from_expr),
        color: named_arg(args, "color").and_then(render_color_from_expr),
        children,
    }
}

fn render_bounds_from_args(args: &[HirCallArg]) -> Option<IrRenderBounds> {
    Some(IrRenderBounds {
        x: named_arg(args, "x").and_then(render_number_from_expr)?,
        y: named_arg(args, "y").and_then(render_number_from_expr)?,
        width: named_arg(args, "width").and_then(render_number_from_expr)?,
        height: named_arg(args, "height").and_then(render_number_from_expr)?,
    })
}

fn render_number_from_expr(expr: &HirExpr) -> Option<IrRenderNumber> {
    match &expr.kind {
        HirExprKind::Literal {
            literal: HirLiteral::Number { value },
        } => Some(IrRenderNumber::Literal { value: *value }),
        HirExprKind::Path { value } => Some(IrRenderNumber::Binding {
            path: value.clone(),
        }),
        _ => None,
    }
}

fn render_color_from_expr(expr: &HirExpr) -> Option<[u8; 4]> {
    let HirExprKind::Literal {
        literal: HirLiteral::Text { value },
    } = &expr.kind
    else {
        return None;
    };
    parse_hex_color(value)
}

fn parse_hex_color(value: &str) -> Option<[u8; 4]> {
    let hex = value.trim().strip_prefix('#')?;
    let (rgb, alpha) = match hex.len() {
        6 => (hex, 255),
        8 => (&hex[..6], u8::from_str_radix(&hex[6..8], 16).ok()?),
        _ => return None,
    };
    let red = u8::from_str_radix(&rgb[0..2], 16).ok()?;
    let green = u8::from_str_radix(&rgb[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&rgb[4..6], 16).ok()?;
    Some([red, green, blue, alpha])
}

fn render_pipeline_node(
    input: &HirExpr,
    stages: &[HirExpr],
    sources: &SourceInventory,
    ids: &mut RenderIdAllocator,
) -> IrRenderNode {
    if let Some(stage) = stages.iter().find(|stage| {
        matches!(
            stage.kind,
            HirExprKind::ListCall {
                op: HirListOp::Map,
                ..
            }
        )
    }) && let HirExprKind::ListCall { args, .. } = &stage.kind
    {
        return IrRenderNode {
            id: ids.next("list_map"),
            kind: IrRenderKind::ListMap,
            source_path: None,
            collection_path: first_path_in_expr(input).map(str::to_string),
            text: None,
            bounds: None,
            scale: None,
            color: None,
            children: named_arg(args, "new")
                .map(|item| vec![render_node_from_expr(item, sources, ids)])
                .unwrap_or_default(),
        };
    }
    IrRenderNode {
        id: ids.next("binding"),
        kind: IrRenderKind::Text,
        source_path: None,
        collection_path: None,
        text: first_path_in_expr(input).map(|path| IrRenderText::Binding {
            path: path.to_string(),
        }),
        bounds: None,
        scale: None,
        color: None,
        children: Vec::new(),
    }
}

fn render_children_from_expr(
    expr: &HirExpr,
    sources: &SourceInventory,
    ids: &mut RenderIdAllocator,
) -> Vec<IrRenderNode> {
    match &expr.kind {
        HirExprKind::List { items } => items
            .iter()
            .map(|item| render_node_from_expr(item, sources, ids))
            .collect(),
        _ => vec![render_node_from_expr(expr, sources, ids)],
    }
}

fn render_text_from_expr(expr: &HirExpr) -> Option<IrRenderText> {
    match &expr.kind {
        HirExprKind::Literal {
            literal: HirLiteral::Text { value },
        } => Some(IrRenderText::Literal {
            value: value.clone(),
        }),
        HirExprKind::Path { value } => Some(IrRenderText::Binding {
            path: value.clone(),
        }),
        HirExprKind::Pipeline { input, .. } => {
            first_path_in_expr(input).map(|path| IrRenderText::Binding {
                path: path.to_string(),
            })
        }
        _ => None,
    }
}

fn resolve_source_base(sources: &SourceInventory, path: &str) -> Option<String> {
    if sources.entries.iter().any(|entry| entry.path == path) {
        return Some(path.to_string());
    }
    if sources
        .entries
        .iter()
        .any(|entry| entry.path.starts_with(&format!("{path}.")))
    {
        return Some(path.to_string());
    }
    let suffix = path
        .strip_prefix("sources.")
        .or_else(|| path.split_once('.').map(|(_, suffix)| suffix))?;
    let suffix = format!(".{suffix}.");
    sources
        .entries
        .iter()
        .filter(|entry| entry.path.contains("[*]"))
        .find_map(|entry| {
            entry
                .path
                .find(&suffix)
                .map(|idx| entry.path[..idx + suffix.len() - 1].to_string())
        })
}

fn record_value_is_list(record: &HirRecord) -> bool {
    record.value.as_ref().is_some_and(matches_list_pipeline)
}

fn push_collection_state_from_hir_record(ir: &mut AppIr, hir: &HirModule, record: &HirRecord) {
    let Some(expr) = record.value.as_ref() else {
        return;
    };
    let Some(entries) = initial_list_seeds(expr, hir) else {
        return;
    };
    ir.collection_states.push(IrCollectionState {
        path: record.key.clone(),
        initial_entries: entries,
    });
}

fn push_derived_value_from_hir_record(ir: &mut AppIr, record: &HirRecord) {
    let Some(expr) = record.value.as_ref() else {
        return;
    };
    let Some(expr) = derived_expr_from_hir(expr) else {
        return;
    };
    ir.derived_values.push(IrDerivedValue {
        path: record.key.clone(),
        expr,
    });
}

fn derived_expr_from_hir(expr: &HirExpr) -> Option<IrDerivedExpr> {
    match &expr.kind {
        HirExprKind::Pipeline { input, stages } => derived_list_pipeline_expr(input, stages),
        HirExprKind::Binary { op, left, right } => {
            let left = path_expr(left)?.to_string();
            let right = path_expr(right)?.to_string();
            match op {
                boon_syntax::AstBinaryOp::Subtract => Some(IrDerivedExpr::Subtract { left, right }),
                boon_syntax::AstBinaryOp::Equal => Some(IrDerivedExpr::Equal { left, right }),
                boon_syntax::AstBinaryOp::Add => None,
            }
        }
        _ => None,
    }
}

fn derived_list_pipeline_expr(input: &HirExpr, stages: &[HirExpr]) -> Option<IrDerivedExpr> {
    let collection_path = path_expr(input)?.to_string();
    match stages {
        [stage]
            if matches!(
                stage.kind,
                HirExprKind::ListCall {
                    op: HirListOp::Count,
                    ..
                }
            ) =>
        {
            Some(IrDerivedExpr::CollectionCount { collection_path })
        }
        [retain, count]
            if matches!(
                count.kind,
                HirExprKind::ListCall {
                    op: HirListOp::Count,
                    ..
                }
            ) =>
        {
            let HirExprKind::ListCall {
                op: HirListOp::Retain,
                args,
            } = &retain.kind
            else {
                return None;
            };
            Some(IrDerivedExpr::CollectionCountWhere {
                collection_path,
                predicate: retain_predicate_from_args(args)?,
            })
        }
        _ => None,
    }
}

fn retain_predicate_from_args(args: &[HirCallArg]) -> Option<IrCollectionPredicate> {
    let predicate = named_arg(args, "if")?;
    let value = path_expr(predicate)?;
    let (_, field) = value.split_once('.')?;
    Some(IrCollectionPredicate::FieldBoolEquals {
        field: field.to_string(),
        value: true,
    })
}

fn expression_surface_from_hir(
    hir: &HirModule,
    sources: &SourceInventory,
) -> Option<IrExpressionSurface> {
    let document = hir_record(hir, "document")?;
    let grid_args = find_host_call_args(document.value.as_ref()?, "Element/grid")?;
    let expressions = named_arg(grid_args, "expressions")
        .and_then(path_expr)
        .and_then(|path| hir_record(hir, path))?;
    let functions = hir_child_record(expressions, "functions")
        .map(|record| {
            record
                .children
                .iter()
                .filter_map(|child| match child.value.as_ref().map(|expr| &expr.kind) {
                    Some(HirExprKind::FunctionCall { path, .. }) if path.starts_with("Math/") => {
                        Some(child.key.clone())
                    }
                    Some(HirExprKind::Path { value }) if value.starts_with("Math/") => {
                        Some(child.key.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let root = dynamic_source_families(sources)
        .first()
        .map(|family| dynamic_family_root(family))?;
    Some(IrExpressionSurface {
        root,
        rows: named_arg(grid_args, "rows")
            .and_then(path_expr)
            .and_then(|path| static_range_end(hir, path))
            .unwrap_or(100),
        columns: named_arg(grid_args, "columns")
            .and_then(path_expr)
            .and_then(|path| static_range_end(hir, path))
            .unwrap_or(26),
        functions,
    })
}

fn find_host_call_args<'a>(expr: &'a HirExpr, path: &str) -> Option<&'a [HirCallArg]> {
    match &expr.kind {
        HirExprKind::HostCall {
            path: call_path,
            args,
        }
        | HirExprKind::FunctionCall {
            path: call_path,
            args,
        } if call_path == path => Some(args),
        HirExprKind::HostCall { args, .. } | HirExprKind::FunctionCall { args, .. } => args
            .iter()
            .find_map(|arg| find_host_call_args(&arg.value, path)),
        HirExprKind::List { items } => items
            .iter()
            .find_map(|item| find_host_call_args(item, path)),
        HirExprKind::Record { entries } => entries.iter().find_map(|entry| {
            entry
                .value
                .as_ref()
                .and_then(|value| find_host_call_args(value, path))
        }),
        HirExprKind::Pipeline { input, stages } => find_host_call_args(input, path).or_else(|| {
            stages
                .iter()
                .find_map(|stage| find_host_call_args(stage, path))
        }),
        HirExprKind::ListCall { args, .. } => args
            .iter()
            .find_map(|arg| find_host_call_args(&arg.value, path)),
        HirExprKind::Block { bindings } => bindings
            .iter()
            .find_map(|binding| find_host_call_args(&binding.value, path)),
        HirExprKind::Binary { left, right, .. } => {
            find_host_call_args(left, path).or_else(|| find_host_call_args(right, path))
        }
        HirExprKind::When { arms } | HirExprKind::While { arms } => arms
            .iter()
            .find_map(|arm| find_host_call_args(&arm.value, path)),
        HirExprKind::Then { body } | HirExprKind::Hold { body, .. } => {
            find_host_call_args(body, path)
        }
        HirExprKind::Latest { branches } => branches
            .iter()
            .find_map(|branch| find_host_call_args(branch, path)),
        HirExprKind::Source
        | HirExprKind::Literal { .. }
        | HirExprKind::Path { .. }
        | HirExprKind::Tag { .. }
        | HirExprKind::Passed
        | HirExprKind::Skip
        | HirExprKind::Unsupported { .. } => None,
    }
}

fn static_range_end(hir: &HirModule, record_name: &str) -> Option<usize> {
    let record = hir_record(hir, record_name)?;
    let HirExprKind::ListCall {
        op: HirListOp::Range,
        args,
    } = &record.value.as_ref()?.kind
    else {
        return None;
    };
    named_arg(args, "to")
        .and_then(number_literal)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
}

fn initial_list_seeds(expr: &HirExpr, hir: &HirModule) -> Option<Vec<IrCollectionSeed>> {
    let list_expr = match &expr.kind {
        HirExprKind::List { items } => items,
        HirExprKind::Pipeline { input, .. } => match &input.kind {
            HirExprKind::List { items } => items,
            _ => return None,
        },
        _ => return None,
    };
    Some(
        list_expr
            .iter()
            .map(|item| IrCollectionSeed {
                fields: collection_seed_fields_from_expr(item, hir),
            })
            .collect(),
    )
}

fn collection_seed_fields_from_expr(expr: &HirExpr, hir: &HirModule) -> Vec<IrStaticField> {
    match &expr.kind {
        HirExprKind::FunctionCall { path, args } => hir_function(hir, path)
            .and_then(|function| record_fields_from_function_call(function, args))
            .unwrap_or_default(),
        HirExprKind::Record { entries } => literal_record_fields(entries),
        _ => Vec::new(),
    }
}

fn record_fields_from_function_call(
    function: &boon_hir::HirFunction,
    args: &[HirCallArg],
) -> Option<Vec<IrStaticField>> {
    let HirExprKind::Record { entries } = &function.body.kind else {
        return None;
    };
    let mut env = BTreeMap::new();
    for arg in args {
        if let Some(value) = static_value_from_expr(&arg.value) {
            env.insert(arg.name.clone(), value);
        }
    }
    Some(literal_record_fields_with_env(entries, &env))
}

fn literal_record_fields_with_env(
    records: &[HirRecord],
    env: &BTreeMap<String, IrStaticValue>,
) -> Vec<IrStaticField> {
    records
        .iter()
        .filter(|record| record.key != "sources")
        .filter_map(|record| {
            static_value_from_record_with_env(record, env).map(|value| IrStaticField {
                key: record.key.clone(),
                value,
            })
        })
        .collect()
}

fn static_value_from_record_with_env(
    record: &HirRecord,
    env: &BTreeMap<String, IrStaticValue>,
) -> Option<IrStaticValue> {
    if !record.children.is_empty() {
        return Some(IrStaticValue::Record {
            fields: literal_record_fields_with_env(&record.children, env),
        });
    }
    record
        .value
        .as_ref()
        .and_then(|value| static_value_from_expr_with_env(value, env))
}

fn static_value_from_expr_with_env(
    expr: &HirExpr,
    env: &BTreeMap<String, IrStaticValue>,
) -> Option<IrStaticValue> {
    match &expr.kind {
        HirExprKind::Path { value } => env.get(value).cloned(),
        HirExprKind::Pipeline { input, stages }
            if stages
                .iter()
                .any(|stage| matches!(stage.kind, HirExprKind::Hold { .. })) =>
        {
            static_value_from_expr_with_env(input, env)
        }
        _ => static_value_from_expr(expr),
    }
}

fn hir_function<'a>(hir: &'a HirModule, name: &str) -> Option<&'a boon_hir::HirFunction> {
    hir.items.iter().find_map(|item| match item {
        HirItem::Function(function) if function.name == name => Some(function),
        _ => None,
    })
}

fn push_static_record_from_hir_record(ir: &mut AppIr, record: &HirRecord) {
    if record.key == "store" || record.key == "document" {
        return;
    }
    let fields = if record.children.is_empty() {
        record
            .value
            .as_ref()
            .and_then(static_value_from_expr)
            .map(|value| {
                vec![IrStaticField {
                    key: "value".to_string(),
                    value,
                }]
            })
            .unwrap_or_default()
    } else {
        literal_record_fields(&record.children)
    };
    if fields.is_empty() {
        return;
    }
    ir.static_records.push(IrStaticRecord {
        path: record.key.clone(),
        fields,
    });
}

fn literal_record_fields(records: &[HirRecord]) -> Vec<IrStaticField> {
    records
        .iter()
        .filter_map(|record| {
            static_value_from_record(record).map(|value| IrStaticField {
                key: record.key.clone(),
                value,
            })
        })
        .collect()
}

fn static_value_from_record(record: &HirRecord) -> Option<IrStaticValue> {
    if !record.children.is_empty() {
        return Some(IrStaticValue::Record {
            fields: literal_record_fields(&record.children),
        });
    }
    record.value.as_ref().and_then(static_value_from_expr)
}

fn static_value_from_expr(expr: &HirExpr) -> Option<IrStaticValue> {
    match &expr.kind {
        HirExprKind::Literal {
            literal: HirLiteral::Text { value },
        } => Some(IrStaticValue::Text {
            value: value.clone(),
        }),
        HirExprKind::Path { value } => Some(IrStaticValue::Path {
            value: value.clone(),
        }),
        HirExprKind::Literal {
            literal: HirLiteral::Number { value },
        } => Some(IrStaticValue::Number { value: *value }),
        HirExprKind::Literal {
            literal: HirLiteral::Bool { value },
        } => Some(IrStaticValue::Bool { value: *value }),
        HirExprKind::Tag { value } => Some(IrStaticValue::Tag {
            value: value.clone(),
        }),
        HirExprKind::List { items } => Some(IrStaticValue::List {
            items: items.iter().filter_map(static_value_from_expr).collect(),
        }),
        HirExprKind::Record { entries } => Some(IrStaticValue::Record {
            fields: literal_record_fields(entries),
        }),
        HirExprKind::ListCall {
            op: HirListOp::Range,
            args,
        } => Some(IrStaticValue::Range {
            from: named_arg(args, "from")
                .and_then(number_literal)
                .unwrap_or(1),
            to: named_arg(args, "to").and_then(number_literal).unwrap_or(0),
        }),
        _ => None,
    }
}

fn matches_list_pipeline(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::List { .. } => true,
        HirExprKind::Pipeline { input, stages } => {
            matches_list_pipeline(input)
                || stages.iter().any(|stage| {
                    matches!(
                        stage.kind,
                        HirExprKind::ListCall {
                            op: HirListOp::Append
                                | HirListOp::Remove
                                | HirListOp::Retain
                                | HirListOp::Map
                                | HirListOp::Count
                                | HirListOp::Range,
                            ..
                        }
                    )
                })
        }
        _ => false,
    }
}

fn accumulator_from_hir_record(
    record: &HirRecord,
    sources: &SourceInventory,
) -> Option<(String, i64, i64)> {
    let expr = record.value.as_ref()?;
    let (input, stages) = pipeline_parts(expr)?;
    let initial = number_literal(input)?;
    let (state_name, hold_body) = stages.iter().find_map(hold_stage)?;
    let (event_path, step) = source_then_add_step(hold_body, state_name, sources)?;
    if event_path.ends_with(".event.frame") {
        return None;
    }
    Some((event_path, initial, step))
}

fn source_then_add_step(
    expr: &HirExpr,
    state_name: &str,
    sources: &SourceInventory,
) -> Option<(String, i64)> {
    match &expr.kind {
        HirExprKind::Pipeline { input, stages } => {
            let event_path = resolve_source_path(sources, path_expr(input)?)?;
            let then = stages.iter().find_map(then_body)?;
            let step = add_step_expr(then, state_name)?;
            Some((event_path, step))
        }
        HirExprKind::Latest { branches } => branches
            .iter()
            .find_map(|branch| source_then_add_step(branch, state_name, sources)),
        _ => None,
    }
}

fn push_collection_handlers_from_hir_record(
    ir: &mut AppIr,
    hir: &HirModule,
    sources: &SourceInventory,
    record: &HirRecord,
) {
    let Some(expr) = record.value.as_ref() else {
        return;
    };
    let Some((_, stages)) = pipeline_parts(expr) else {
        return;
    };
    for stage in stages {
        let HirExprKind::ListCall { op, args } = &stage.kind else {
            continue;
        };
        match op {
            HirListOp::Append => {
                if let Some((key_path, text_path, text_field, default_fields)) =
                    append_text_sources(hir, sources, args)
                {
                    ir.event_handlers.push(IrEventHandler {
                        source_path: key_path.clone(),
                        when: Some(IrPredicate::SourceTagEquals {
                            path: key_path,
                            tag: "Enter".to_string(),
                        }),
                        effects: vec![
                            IrEffect::CollectionAppendRecord {
                                collection_path: record.key.clone(),
                                fields: append_record_fields(
                                    text_field.clone(),
                                    text_path.clone(),
                                    default_fields,
                                ),
                                skip_if_empty_field: Some(text_field),
                            },
                            IrEffect::ClearText {
                                text_state_path: text_path,
                            },
                        ],
                    });
                }
            }
            HirListOp::Remove => {
                if let Some(on_expr) = named_arg(args, "on") {
                    if let Some(dynamic_path) =
                        item_source_path(sources, on_expr).filter(|path| path.contains("[*]"))
                    {
                        ir.event_handlers.push(IrEventHandler {
                            source_path: dynamic_path,
                            when: None,
                            effects: vec![IrEffect::CollectionRemoveCurrent {
                                collection_path: record.key.clone(),
                            }],
                        });
                    } else if let Some(static_path) = static_source_path_in_expr(sources, on_expr)
                        && let Some(field) = first_item_field_in_expr(on_expr)
                    {
                        ir.event_handlers.push(IrEventHandler {
                            source_path: static_path,
                            when: None,
                            effects: vec![IrEffect::CollectionRemoveWhere {
                                collection_path: record.key.clone(),
                                predicate: IrCollectionPredicate::FieldBoolEquals {
                                    field,
                                    value: true,
                                },
                            }],
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

fn append_text_sources(
    hir: &HirModule,
    sources: &SourceInventory,
    args: &[HirCallArg],
) -> Option<(String, String, String, Vec<IrStaticField>)> {
    let item_expr = named_arg(args, "item")?;
    let producer_record_name = first_path_in_expr(item_expr)?;
    let producer = hir_record(hir, producer_record_name)?;
    let (key_path, text_path) = submit_text_sources(producer.value.as_ref()?, sources)?;
    let (text_field, default_fields) = append_record_shape(hir, item_expr)?;
    Some((key_path, text_path, text_field, default_fields))
}

fn append_record_fields(
    text_field: String,
    text_path: String,
    default_fields: Vec<IrStaticField>,
) -> Vec<IrCollectionFieldAssignment> {
    let mut fields = default_fields
        .into_iter()
        .map(|field| IrCollectionFieldAssignment {
            field: field.key,
            value: IrCollectionValueExpr::Static { value: field.value },
        })
        .collect::<Vec<_>>();
    fields.push(IrCollectionFieldAssignment {
        field: text_field,
        value: IrCollectionValueExpr::SourceText {
            path: text_path,
            trim: true,
        },
    });
    fields
}

fn append_record_shape(hir: &HirModule, expr: &HirExpr) -> Option<(String, Vec<IrStaticField>)> {
    let (_, stages) = pipeline_parts(expr)?;
    let append_stage = stages.iter().find_map(|stage| match &stage.kind {
        HirExprKind::FunctionCall { path, args } => {
            let function = hir_function(hir, path)?;
            let HirExprKind::Record { entries } = &function.body.kind else {
                return None;
            };
            let passed_arg = args.iter().find_map(|arg| {
                matches!(arg.value.kind, HirExprKind::Passed).then(|| arg.name.clone())
            })?;
            let mut defaults = Vec::new();
            let mut text_field = None;
            for entry in entries {
                if entry.key == "sources" {
                    continue;
                }
                if entry
                    .value
                    .as_ref()
                    .is_some_and(|value| matches!(value.kind, HirExprKind::Path { ref value } if value == &passed_arg))
                {
                    text_field = Some(entry.key.clone());
                    continue;
                }
                if let Some(value) = static_value_from_record_with_env(entry, &BTreeMap::new()) {
                    defaults.push(IrStaticField {
                        key: entry.key.clone(),
                        value,
                    });
                }
            }
            Some((text_field?, defaults))
        }
        _ => None,
    })?;
    Some(append_stage)
}

fn submit_text_sources(expr: &HirExpr, sources: &SourceInventory) -> Option<(String, String)> {
    let (input, stages) = pipeline_parts(expr)?;
    let key_path = resolve_source_path(sources, path_expr(input)?)?;
    if !matches!(
        stages.first().map(|stage| &stage.kind),
        Some(HirExprKind::When { .. })
    ) {
        return None;
    }
    let text_path = first_text_source_in_expr(stages.first()?, sources)?;
    Some((key_path, text_path))
}

fn push_selector_handlers_from_hir_record(
    ir: &mut AppIr,
    sources: &SourceInventory,
    record: &HirRecord,
) {
    let Some(selectors) = hir_child_record(record, "selectors") else {
        return;
    };
    for selector in &selectors.children {
        let source_path = format!("store.sources.{}.event.press", selector.key);
        if existing_source_path(sources, &source_path).is_some() {
            ir.event_handlers.push(IrEventHandler {
                source_path,
                when: None,
                effects: vec![IrEffect::SetTagState {
                    state_path: format!("{}.selector", record.key),
                    value: selector.key.clone(),
                }],
            });
        }
    }
}

fn push_item_state_handlers_from_hir(
    ir: &mut AppIr,
    sources: &SourceInventory,
    collection_path: &str,
    hir: &HirModule,
) {
    for function in hir.items.iter().filter_map(|item| match item {
        HirItem::Function(function) => Some(function),
        _ => None,
    }) {
        for record in records_in_expr(&function.body) {
            for event_path in latest_then_sources(record.value.as_ref(), sources) {
                let effects = if event_path.contains("[*]") {
                    vec![IrEffect::CollectionUpdateOwnerField {
                        collection_path: collection_path.to_string(),
                        field: record.key.clone(),
                        value: IrCollectionValueExpr::NotOwnerBoolField {
                            field: record.key.clone(),
                        },
                    }]
                } else {
                    vec![IrEffect::CollectionUpdateAllFields {
                        collection_path: collection_path.to_string(),
                        field: record.key.clone(),
                        value: IrCollectionValueExpr::NotAllBoolField {
                            field: record.key.clone(),
                        },
                    }]
                };
                ir.event_handlers.push(IrEventHandler {
                    source_path: event_path,
                    when: None,
                    effects,
                });
            }
        }
    }
}

fn latest_then_sources(expr: Option<&HirExpr>, sources: &SourceInventory) -> Vec<String> {
    let mut paths = Vec::new();
    let Some(expr) = expr else {
        return paths;
    };
    collect_latest_then_sources(expr, sources, &mut paths);
    paths
}

fn collect_latest_then_sources(expr: &HirExpr, sources: &SourceInventory, paths: &mut Vec<String>) {
    match &expr.kind {
        HirExprKind::Pipeline { input, stages } => {
            if stages
                .iter()
                .any(|stage| matches!(stage.kind, HirExprKind::Then { .. }))
                && let Some(path) =
                    path_expr(input).and_then(|path| resolve_source_path(sources, path))
            {
                paths.push(path);
            }
            collect_latest_then_sources(input, sources, paths);
            for stage in stages {
                collect_latest_then_sources(stage, sources, paths);
            }
        }
        HirExprKind::Latest { branches } => {
            for branch in branches {
                collect_latest_then_sources(branch, sources, paths);
            }
        }
        HirExprKind::While { arms } | HirExprKind::When { arms } => {
            for arm in arms {
                collect_latest_then_sources(&arm.value, sources, paths);
            }
        }
        HirExprKind::Then { body } | HirExprKind::Hold { body, .. } => {
            collect_latest_then_sources(body, sources, paths);
        }
        _ => {}
    }
}

fn dedupe_app_ir(ir: &mut AppIr) {
    let mut seen_state = HashSet::new();
    ir.state_cells
        .retain(|cell| seen_state.insert(cell.path.clone()));
    let mut seen_list = HashSet::new();
    ir.collection_states
        .retain(|list| seen_list.insert(list.path.clone()));
    let mut seen_static = HashSet::new();
    ir.static_records
        .retain(|record| seen_static.insert(record.path.clone()));
    let mut seen_handlers = HashSet::new();
    ir.event_handlers.retain(|handler| {
        seen_handlers.insert(serde_json::to_string(handler).expect("app IR handler serializes"))
    });
}

fn pipeline_parts(expr: &HirExpr) -> Option<(&HirExpr, &[HirExpr])> {
    match &expr.kind {
        HirExprKind::Pipeline { input, stages } => Some((input, stages.as_slice())),
        _ => None,
    }
}

fn hold_stage(expr: &HirExpr) -> Option<(&str, &HirExpr)> {
    match &expr.kind {
        HirExprKind::Hold { state, body } => Some((state.as_str(), body)),
        _ => None,
    }
}

fn then_body(expr: &HirExpr) -> Option<&HirExpr> {
    match &expr.kind {
        HirExprKind::Then { body } => Some(body),
        _ => None,
    }
}

fn number_literal(expr: &HirExpr) -> Option<i64> {
    match &expr.kind {
        HirExprKind::Literal {
            literal: HirLiteral::Number { value },
        } => Some(*value),
        _ => None,
    }
}

fn add_step_expr(expr: &HirExpr, state_name: &str) -> Option<i64> {
    match &expr.kind {
        HirExprKind::Binary {
            op: boon_syntax::AstBinaryOp::Add,
            left,
            right,
        } if path_expr(left).is_some_and(|path| path == state_name) => number_literal(right),
        _ => None,
    }
}

fn named_arg<'a>(args: &'a [HirCallArg], name: &str) -> Option<&'a HirExpr> {
    args.iter()
        .find(|arg| arg.name == name)
        .map(|arg| &arg.value)
}

fn path_expr(expr: &HirExpr) -> Option<&str> {
    match &expr.kind {
        HirExprKind::Path { value } => Some(value.as_str()),
        _ => None,
    }
}

fn first_path_in_expr(expr: &HirExpr) -> Option<&str> {
    match &expr.kind {
        HirExprKind::Path { value } => Some(value.as_str()),
        HirExprKind::Pipeline { input, .. } => first_path_in_expr(input),
        _ => None,
    }
}

fn first_text_source_in_expr(expr: &HirExpr, sources: &SourceInventory) -> Option<String> {
    let mut paths = Vec::new();
    collect_expr_paths(expr, &mut paths);
    paths.into_iter().find_map(|path| {
        let resolved = resolve_source_path(sources, &path)?;
        sources
            .entries
            .iter()
            .any(|entry| entry.path == resolved && entry.shape == Shape::Text)
            .then_some(resolved)
    })
}

fn first_item_field_in_expr(expr: &HirExpr) -> Option<String> {
    let mut paths = Vec::new();
    collect_expr_paths(expr, &mut paths);
    paths
        .into_iter()
        .find_map(|path| path.strip_prefix("item.").map(str::to_string))
}

fn static_source_path_in_expr(sources: &SourceInventory, expr: &HirExpr) -> Option<String> {
    let mut paths = Vec::new();
    collect_expr_paths(expr, &mut paths);
    paths.into_iter().find_map(|path| {
        resolve_source_path(sources, &path).filter(|resolved| !resolved.contains("[*]"))
    })
}

fn item_source_path(sources: &SourceInventory, expr: &HirExpr) -> Option<String> {
    let path = path_expr(expr)?;
    resolve_source_path(sources, path)
}

fn resolve_source_path(sources: &SourceInventory, path: &str) -> Option<String> {
    existing_source_path(sources, path).or_else(|| {
        path.strip_prefix("item.")
            .or_else(|| path.strip_prefix("sources."))
            .and_then(|suffix| source_family_with_suffix(sources, suffix))
    })
}

fn source_family_with_suffix(sources: &SourceInventory, suffix: &str) -> Option<String> {
    let suffix = format!(".{suffix}");
    sources
        .entries
        .iter()
        .find(|entry| entry.path.contains("[*]") && entry.path.ends_with(&suffix))
        .map(|entry| entry.path.clone())
}

fn hir_record<'a>(hir: &'a HirModule, key: &str) -> Option<&'a HirRecord> {
    hir.items.iter().find_map(|item| match item {
        HirItem::Record(record) if record.key == key => Some(record),
        _ => None,
    })
}

fn hir_child_record<'a>(record: &'a HirRecord, key: &str) -> Option<&'a HirRecord> {
    record.children.iter().find(|child| child.key == key)
}

fn records_in_expr(expr: &HirExpr) -> Vec<&HirRecord> {
    let mut records = Vec::new();
    collect_records_in_expr(expr, &mut records);
    records
}

fn collect_records_in_expr<'a>(expr: &'a HirExpr, records: &mut Vec<&'a HirRecord>) {
    match &expr.kind {
        HirExprKind::Record { entries } => {
            for entry in entries {
                collect_record_and_children(entry, records);
            }
        }
        HirExprKind::Pipeline { input, stages } => {
            collect_records_in_expr(input, records);
            for stage in stages {
                collect_records_in_expr(stage, records);
            }
        }
        HirExprKind::Then { body } | HirExprKind::Hold { body, .. } => {
            collect_records_in_expr(body, records);
        }
        HirExprKind::Latest { branches } => {
            for branch in branches {
                collect_records_in_expr(branch, records);
            }
        }
        HirExprKind::List { items } => {
            for item in items {
                collect_records_in_expr(item, records);
            }
        }
        HirExprKind::HostCall { args, .. }
        | HirExprKind::ListCall { args, .. }
        | HirExprKind::FunctionCall { args, .. } => {
            for arg in args {
                collect_records_in_expr(&arg.value, records);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_records_in_expr(left, records);
            collect_records_in_expr(right, records);
        }
        HirExprKind::Block { bindings } => {
            for binding in bindings {
                collect_records_in_expr(&binding.value, records);
            }
        }
        HirExprKind::When { arms } | HirExprKind::While { arms } => {
            for arm in arms {
                collect_records_in_expr(&arm.value, records);
            }
        }
        _ => {}
    }
}

fn collect_record_and_children<'a>(record: &'a HirRecord, records: &mut Vec<&'a HirRecord>) {
    records.push(record);
    if let Some(value) = &record.value {
        collect_records_in_expr(value, records);
    }
    for child in &record.children {
        collect_record_and_children(child, records);
    }
}

fn collect_expr_paths(expr: &HirExpr, paths: &mut Vec<String>) {
    match &expr.kind {
        HirExprKind::Path { value } => paths.push(value.clone()),
        HirExprKind::Record { entries } => {
            for entry in entries {
                if let Some(value) = &entry.value {
                    collect_expr_paths(value, paths);
                }
                for child in &entry.children {
                    if let Some(value) = &child.value {
                        collect_expr_paths(value, paths);
                    }
                }
            }
        }
        HirExprKind::List { items } => {
            for item in items {
                collect_expr_paths(item, paths);
            }
        }
        HirExprKind::Block { bindings } => {
            for binding in bindings {
                collect_expr_paths(&binding.value, paths);
            }
        }
        HirExprKind::When { arms } | HirExprKind::While { arms } => {
            for arm in arms {
                collect_expr_paths(&arm.value, paths);
            }
        }
        HirExprKind::Then { body } | HirExprKind::Hold { body, .. } => {
            collect_expr_paths(body, paths);
        }
        HirExprKind::Latest { branches } => {
            for branch in branches {
                collect_expr_paths(branch, paths);
            }
        }
        HirExprKind::HostCall { args, .. }
        | HirExprKind::ListCall { args, .. }
        | HirExprKind::FunctionCall { args, .. } => {
            for arg in args {
                collect_expr_paths(&arg.value, paths);
            }
        }
        HirExprKind::Pipeline { input, stages } => {
            collect_expr_paths(input, paths);
            for stage in stages {
                collect_expr_paths(stage, paths);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_expr_paths(left, paths);
            collect_expr_paths(right, paths);
        }
        _ => {}
    }
}

fn push_accumulator_ir(
    ir: &mut AppIr,
    event_path: &str,
    state_path: &str,
    initial: i64,
    step: i64,
) {
    ir.state_cells.push(IrStateCell {
        path: state_path.to_string(),
        initial: IrValueExpr::Number { value: initial },
    });
    ir.event_handlers.push(IrEventHandler {
        source_path: event_path.to_string(),
        when: None,
        effects: vec![IrEffect::Assign {
            state_path: state_path.to_string(),
            expr: IrValueExpr::Add {
                left: Box::new(IrValueExpr::Hold {
                    state_path: state_path.to_string(),
                }),
                right: Box::new(IrValueExpr::Number { value: step }),
            },
        }],
    });
}

fn existing_source_path(sources: &SourceInventory, path: &str) -> Option<String> {
    sources
        .entries
        .iter()
        .any(|entry| entry.path == path)
        .then(|| path.to_string())
}

fn dynamic_family_root(family: &str) -> String {
    family
        .strip_suffix("[*]")
        .unwrap_or(family)
        .rsplit('.')
        .next()
        .unwrap_or(family)
        .to_string()
}

fn compiled_provenance(source: &str, hir: &HirModule) -> Result<CompiledProvenance> {
    let source_sha256 = hex::encode(Sha256::digest(source.as_bytes()));
    let hir_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(hir)?));
    let mut source_spans = Vec::new();
    for leaf in &hir.parsed.source_leaves {
        source_spans.push(CompiledSourceSpan {
            kind: "SOURCE".to_string(),
            path: leaf.path.clone(),
            line: leaf.span.line,
            column: leaf.span.column,
        });
    }
    for call in &hir.parsed.module_calls {
        source_spans.push(CompiledSourceSpan {
            kind: "module_call".to_string(),
            path: call.path.clone(),
            line: call.span.line,
            column: call.span.column,
        });
    }
    Ok(CompiledProvenance {
        source_sha256,
        hir_sha256,
        source_spans,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HostBinding {
    function: String,
    source_base: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceBinding {
    function: String,
    relative_path: String,
    shape: Shape,
}

fn collect_host_bindings(
    parsed: &ParsedModule,
    map_alias_roots: &BTreeMap<String, String>,
) -> Vec<HostBinding> {
    parsed
        .module_calls
        .iter()
        .filter(|call| call.path.starts_with("Element/"))
        .flat_map(|call| {
            call.args
                .iter()
                .filter(|arg| arg.name == "element")
                .map(|arg| HostBinding {
                    function: call.path.clone(),
                    source_base: normalize_binding_expr(&arg.value, map_alias_roots),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn normalize_binding_expr(expr: &str, map_alias_roots: &BTreeMap<String, String>) -> String {
    if let Some((alias, tail)) = expr.split_once(".sources.")
        && let Some(root) = map_alias_roots.get(alias)
    {
        format!("{root}[*].sources.{tail}")
    } else if let Some((root, tail)) = expr.split_once(".sources.") {
        if root == "store" {
            expr.to_string()
        } else {
            format!("{root}[*].sources.{tail}")
        }
    } else {
        expr.to_string()
    }
}

fn map_alias_roots(
    parsed: &ParsedModule,
    dynamic_sequence_root: Option<&str>,
    dynamic_mapped_root: Option<&str>,
) -> BTreeMap<String, String> {
    let mut roots = BTreeMap::new();
    for binding in &parsed.map_bindings {
        let collection = binding.collection_root();
        let root = collection
            .and_then(|collection| roots.get(collection).cloned())
            .or_else(|| {
                collection
                    .zip(dynamic_sequence_root)
                    .and_then(|(collection, root)| (collection == root).then(|| root.to_string()))
            })
            .or_else(|| {
                collection
                    .zip(dynamic_mapped_root)
                    .and_then(|(collection, root)| (collection == root).then(|| root.to_string()))
            })
            .or_else(|| {
                (dynamic_sequence_root.is_some() && dynamic_mapped_root.is_none())
                    .then(|| dynamic_sequence_root.expect("checked").to_string())
            })
            .or_else(|| {
                (dynamic_mapped_root.is_some() && dynamic_sequence_root.is_none())
                    .then(|| dynamic_mapped_root.expect("checked").to_string())
            })
            .or_else(|| Some("records".to_string()));
        if let Some(root) = root {
            roots.insert(binding.variable.clone(), root);
        }
    }
    roots
}

trait MapBindingRoot {
    fn collection_root(&self) -> Option<&str>;
}

impl MapBindingRoot for boon_syntax::MapBinding {
    fn collection_root(&self) -> Option<&str> {
        self.collection
            .split(|ch: char| ch.is_whitespace() || ch == '.' || ch == '|')
            .find(|part| !part.is_empty())
    }
}

fn binding_for_source(
    source_path: &str,
    host_bindings: &[HostBinding],
    contracts: &[HostContract],
) -> Result<SourceBinding> {
    let mut matches = Vec::new();
    for binding in host_bindings {
        let Some(relative_path) = source_path
            .strip_prefix(&binding.source_base)
            .and_then(|tail| tail.strip_prefix('.'))
        else {
            continue;
        };
        let Some(contract) = contracts
            .iter()
            .find(|contract| contract.function == binding.function)
        else {
            continue;
        };
        if let Some(shape) = contract.accepts(relative_path) {
            matches.push(SourceBinding {
                function: binding.function.clone(),
                relative_path: relative_path.to_string(),
                shape: shape.clone(),
            });
        }
    }
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => bail!(
            "SOURCE path `{source_path}` has no statically provable Element binding with a compatible host contract"
        ),
        _ => bail!("SOURCE path `{source_path}` is bound by more than one Element producer"),
    }
}

fn app_metadata(name: &str, parsed: &ParsedModule) -> IrAppMetadata {
    let title = first_child_text(parsed, "title").unwrap_or_else(|| name.replace('_', " "));
    IrAppMetadata {
        title,
        primary_label: scalar_button_label(parsed),
        physical_debug: record_bool(top_record(parsed, "view"), "physical_debug").unwrap_or(false),
    }
}

fn dynamic_source_families(sources: &SourceInventory) -> Vec<String> {
    let mut families = Vec::new();
    for entry in &sources.entries {
        let Some((family, _)) = entry.path.split_once(".sources.") else {
            continue;
        };
        if family.contains("[*]") && !families.iter().any(|existing| existing == family) {
            families.push(family.to_string());
        }
    }
    families
}

fn infer_dynamic_sequence_root(parsed: &ParsedModule) -> Option<String> {
    let has_append = module_called(parsed, "List/append");
    parsed
        .records
        .iter()
        .find(|record| {
            has_append
                && record.key != "store"
                && module_called_under_record(parsed, record, "List/append")
        })
        .map(|record| record.key.clone())
}

fn infer_dynamic_mapped_source_root(parsed: &ParsedModule) -> Option<String> {
    let mapped_collections = parsed
        .map_bindings
        .iter()
        .filter_map(|binding| binding.collection_root())
        .collect::<HashSet<_>>();
    parsed
        .records
        .iter()
        .find(|record| {
            record.key != "store"
                && child_record(record, "sources").is_some()
                && (mapped_collections.contains(record.key.as_str())
                    || ast_references_path(parsed, &format!("{}.sources", record.key)))
        })
        .map(|record| record.key.clone())
}

fn ast_references_path(parsed: &ParsedModule, path: &str) -> bool {
    parsed
        .ast
        .items
        .iter()
        .any(|item| ast_item_references_path(item, path))
}

fn ast_item_references_path(item: &AstItem, path: &str) -> bool {
    match item {
        AstItem::Record(record) => ast_record_references_path(record, path),
        AstItem::Function(function) => ast_expr_references_path(&function.body, path),
        AstItem::Expression(expr) => ast_expr_references_path(expr, path),
    }
}

fn ast_record_references_path(record: &AstRecord, path: &str) -> bool {
    record
        .value
        .as_ref()
        .is_some_and(|expr| ast_expr_references_path(expr, path))
        || record
            .children
            .iter()
            .any(|child| ast_record_references_path(child, path))
}

fn ast_expr_references_path(expr: &AstExpr, path: &str) -> bool {
    match &expr.kind {
        AstExprKind::Path { value } => value == path,
        AstExprKind::Record { entries } => entries
            .iter()
            .any(|record| ast_record_references_path(record, path)),
        AstExprKind::List { items } => items
            .iter()
            .any(|item| ast_expr_references_path(item, path)),
        AstExprKind::Block { bindings } => bindings
            .iter()
            .any(|binding| ast_expr_references_path(&binding.value, path)),
        AstExprKind::When { arms } | AstExprKind::While { arms } => arms
            .iter()
            .any(|arm| ast_expr_references_path(&arm.value, path)),
        AstExprKind::Then { body } | AstExprKind::Hold { body, .. } => {
            ast_expr_references_path(body, path)
        }
        AstExprKind::Latest { branches } => branches
            .iter()
            .any(|branch| ast_expr_references_path(branch, path)),
        AstExprKind::Call { args, .. } => args
            .iter()
            .any(|arg| ast_expr_references_path(&arg.value, path)),
        AstExprKind::Pipeline { input, stages } => {
            ast_expr_references_path(input, path)
                || stages
                    .iter()
                    .any(|stage| ast_expr_references_path(stage, path))
        }
        AstExprKind::Binary { left, right, .. } => {
            ast_expr_references_path(left, path) || ast_expr_references_path(right, path)
        }
        AstExprKind::Source
        | AstExprKind::Number { .. }
        | AstExprKind::Bool { .. }
        | AstExprKind::Tag { .. }
        | AstExprKind::Text { .. }
        | AstExprKind::Passed
        | AstExprKind::Skip
        | AstExprKind::Raw => false,
    }
}

fn normalize_source_path(path: &str, dynamic_sequence_root: Option<&str>) -> String {
    if path.starts_with("store.sources.") {
        path.to_string()
    } else if let Some(tail) = path.strip_prefix("sources.") {
        let root = dynamic_sequence_root.unwrap_or("records");
        format!("{root}[*].sources.{tail}")
    } else if let Some((root, tail)) = path.split_once(".sources.") {
        if root == "store" {
            path.to_string()
        } else {
            format!("{root}[*].sources.{tail}")
        }
    } else {
        path.to_string()
    }
}

fn owner_path(path: &str) -> String {
    path.split_once("[*]")
        .map(|(root, _)| format!("{root} record"))
        .unwrap_or_else(|| "dynamic record".to_string())
}

fn module_called_under_record(
    parsed: &ParsedModule,
    record: &ParsedRecordEntry,
    path: &str,
) -> bool {
    parsed
        .module_calls
        .iter()
        .any(|call| call.path == path && span_under_record(parsed, call.span.line, record))
}

fn span_under_record(parsed: &ParsedModule, line: usize, record: &ParsedRecordEntry) -> bool {
    let start_line = record.span.line;
    let end_line = top_record_end_line(parsed, record).unwrap_or(usize::MAX);
    line >= start_line && line < end_line
}

fn top_record_end_line(parsed: &ParsedModule, record: &ParsedRecordEntry) -> Option<usize> {
    parsed
        .records
        .iter()
        .filter(|candidate| candidate.span.line > record.span.line)
        .map(|candidate| candidate.span.line)
        .min()
}

fn child_text(
    parsed: &ParsedModule,
    record: &ParsedRecordEntry,
    child_key: &str,
) -> Option<String> {
    child_record(record, child_key).and_then(|child| text_literal_on_line(parsed, child.span.line))
}

fn scalar_button_label(parsed: &ParsedModule) -> Option<String> {
    top_record(parsed, "view")
        .and_then(|view| child_record(view, "action"))
        .and_then(|action| child_text(parsed, action, "label"))
}

fn first_child_text(parsed: &ParsedModule, field: &str) -> Option<String> {
    parsed
        .records
        .iter()
        .find_map(|record| child_record(record, field))
        .and_then(|child| text_literal_on_line(parsed, child.span.line))
}

fn text_literal_on_line(parsed: &ParsedModule, line: usize) -> Option<String> {
    parsed
        .text_literals
        .iter()
        .find(|literal| literal.span.line == line)
        .map(|literal| literal.value.clone())
}

fn top_record<'a>(parsed: &'a ParsedModule, key: &str) -> Option<&'a ParsedRecordEntry> {
    parsed.records.iter().find(|entry| entry.key == key)
}

fn child_record<'a>(record: &'a ParsedRecordEntry, key: &str) -> Option<&'a ParsedRecordEntry> {
    record.children.iter().find(|entry| entry.key == key)
}

fn record_bool(record: Option<&ParsedRecordEntry>, field: &str) -> Option<bool> {
    match child_record(record?, field)?.value.as_deref()?.trim() {
        "True" => Some(true),
        "False" => Some(false),
        _ => None,
    }
}

fn module_called(parsed: &ParsedModule, path: &str) -> bool {
    parsed.module_calls.iter().any(|call| call.path == path)
}
