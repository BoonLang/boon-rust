use boon_syntax::{
    AstBinaryOp, AstExpr, AstExprKind, AstItem, AstModule, AstRecord, ParsedModule, Span,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HirModule {
    pub parsed: ParsedModule,
    pub items: Vec<HirItem>,
    pub features: Vec<HirFeature>,
    pub dependencies: Vec<HirDependency>,
    pub diagnostics: Vec<HirDiagnostic>,
}

pub fn lower(parsed: ParsedModule) -> HirModule {
    let mut ctx = LowerContext::default();
    let items = lower_ast_module(&parsed.ast, &mut ctx);
    HirModule {
        parsed,
        items,
        features: ctx.features.into_iter().collect(),
        dependencies: ctx.dependencies,
        diagnostics: ctx.diagnostics,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HirItem {
    Record(HirRecord),
    Function(HirFunction),
    Expression(HirExpr),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HirRecord {
    pub key: String,
    pub value: Option<HirExpr>,
    pub span: Span,
    pub children: Vec<HirRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HirFunction {
    pub name: String,
    pub params: Vec<String>,
    pub body: HirExpr,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HirExpr {
    pub kind: HirExprKind,
    pub span: Span,
    pub raw: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HirExprKind {
    Source,
    Literal {
        literal: HirLiteral,
    },
    Path {
        value: String,
    },
    Tag {
        value: String,
    },
    Passed,
    Skip,
    Record {
        entries: Vec<HirRecord>,
    },
    List {
        items: Vec<HirExpr>,
    },
    Block {
        bindings: Vec<HirBinding>,
    },
    When {
        arms: Vec<HirWhenArm>,
    },
    While {
        arms: Vec<HirWhenArm>,
    },
    Then {
        body: Box<HirExpr>,
    },
    Hold {
        state: String,
        body: Box<HirExpr>,
    },
    Latest {
        branches: Vec<HirExpr>,
    },
    HostCall {
        path: String,
        args: Vec<HirCallArg>,
    },
    ListCall {
        op: HirListOp,
        args: Vec<HirCallArg>,
    },
    FunctionCall {
        path: String,
        args: Vec<HirCallArg>,
    },
    Pipeline {
        input: Box<HirExpr>,
        stages: Vec<HirExpr>,
    },
    Binary {
        op: AstBinaryOp,
        left: Box<HirExpr>,
        right: Box<HirExpr>,
    },
    Unsupported {
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HirLiteral {
    Number { value: i64 },
    Bool { value: bool },
    Text { value: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HirBinding {
    pub name: String,
    pub value: HirExpr,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HirCallArg {
    pub name: String,
    pub value: HirExpr,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HirWhenArm {
    pub pattern: String,
    pub value: HirExpr,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HirFeature {
    Source,
    Hold,
    Then,
    When,
    While,
    Latest,
    Block,
    ListLiteral,
    ListAppend,
    ListRemove,
    ListRetain,
    ListMap,
    ListCount,
    ListRange,
    HostElement,
    HostDocument,
    MathCall,
    FunctionDefinition,
    FunctionCall,
    Pipeline,
    Binary,
    RenderExpression,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HirDependency {
    pub from: String,
    pub to: String,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HirDiagnostic {
    pub span: Span,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HirListOp {
    Append,
    Remove,
    Retain,
    Map,
    Count,
    Range,
    Unknown { path: String },
}

#[derive(Default)]
struct LowerContext {
    features: BTreeSet<HirFeature>,
    dependencies: Vec<HirDependency>,
    diagnostics: Vec<HirDiagnostic>,
}

fn lower_ast_module(module: &AstModule, ctx: &mut LowerContext) -> Vec<HirItem> {
    module
        .items
        .iter()
        .map(|item| match item {
            AstItem::Record(record) => HirItem::Record(lower_record(record, ctx)),
            AstItem::Function(function) => {
                ctx.features.insert(HirFeature::FunctionDefinition);
                HirItem::Function(HirFunction {
                    name: function.name.clone(),
                    params: function.params.clone(),
                    body: lower_expr(&function.body, ctx),
                    span: function.span,
                })
            }
            AstItem::Expression(expr) => HirItem::Expression(lower_expr(expr, ctx)),
        })
        .collect()
}

fn lower_record(record: &AstRecord, ctx: &mut LowerContext) -> HirRecord {
    if record.key == "document" || record.key == "view" {
        ctx.features.insert(HirFeature::RenderExpression);
    }
    let value = record.value.as_ref().map(|expr| lower_expr(expr, ctx));
    if let Some(value) = &value {
        collect_record_dependency(&record.key, value, ctx);
    }
    HirRecord {
        key: record.key.clone(),
        value,
        span: record.span,
        children: record
            .children
            .iter()
            .map(|child| lower_record(child, ctx))
            .collect(),
    }
}

fn lower_expr(expr: &AstExpr, ctx: &mut LowerContext) -> HirExpr {
    let kind = match &expr.kind {
        AstExprKind::Source => {
            ctx.features.insert(HirFeature::Source);
            HirExprKind::Source
        }
        AstExprKind::Number { value } => HirExprKind::Literal {
            literal: HirLiteral::Number { value: *value },
        },
        AstExprKind::Bool { value } => HirExprKind::Literal {
            literal: HirLiteral::Bool { value: *value },
        },
        AstExprKind::Text { value } => HirExprKind::Literal {
            literal: HirLiteral::Text {
                value: value.clone(),
            },
        },
        AstExprKind::Path { value } => HirExprKind::Path {
            value: value.clone(),
        },
        AstExprKind::Tag { value } => HirExprKind::Tag {
            value: value.clone(),
        },
        AstExprKind::Passed => HirExprKind::Passed,
        AstExprKind::Skip => HirExprKind::Skip,
        AstExprKind::Record { entries } => HirExprKind::Record {
            entries: entries
                .iter()
                .map(|entry| lower_record(entry, ctx))
                .collect(),
        },
        AstExprKind::List { items } => {
            ctx.features.insert(HirFeature::ListLiteral);
            HirExprKind::List {
                items: items.iter().map(|item| lower_expr(item, ctx)).collect(),
            }
        }
        AstExprKind::Block { bindings } => {
            ctx.features.insert(HirFeature::Block);
            HirExprKind::Block {
                bindings: bindings
                    .iter()
                    .map(|binding| HirBinding {
                        name: binding.name.clone(),
                        value: lower_expr(&binding.value, ctx),
                        span: binding.span,
                    })
                    .collect(),
            }
        }
        AstExprKind::When { arms } => {
            ctx.features.insert(HirFeature::When);
            HirExprKind::When {
                arms: arms
                    .iter()
                    .map(|arm| HirWhenArm {
                        pattern: arm.pattern.clone(),
                        value: lower_expr(&arm.value, ctx),
                        span: arm.span,
                    })
                    .collect(),
            }
        }
        AstExprKind::While { arms } => {
            ctx.features.insert(HirFeature::While);
            HirExprKind::While {
                arms: arms
                    .iter()
                    .map(|arm| HirWhenArm {
                        pattern: arm.pattern.clone(),
                        value: lower_expr(&arm.value, ctx),
                        span: arm.span,
                    })
                    .collect(),
            }
        }
        AstExprKind::Then { body } => {
            ctx.features.insert(HirFeature::Then);
            HirExprKind::Then {
                body: Box::new(lower_expr(body, ctx)),
            }
        }
        AstExprKind::Hold { state, body } => {
            ctx.features.insert(HirFeature::Hold);
            HirExprKind::Hold {
                state: state.clone(),
                body: Box::new(lower_expr(body, ctx)),
            }
        }
        AstExprKind::Latest { branches } => {
            ctx.features.insert(HirFeature::Latest);
            HirExprKind::Latest {
                branches: branches
                    .iter()
                    .map(|branch| lower_expr(branch, ctx))
                    .collect(),
            }
        }
        AstExprKind::Call { path, args } => lower_call(path, args, expr.span, ctx),
        AstExprKind::Pipeline { input, stages } => {
            ctx.features.insert(HirFeature::Pipeline);
            HirExprKind::Pipeline {
                input: Box::new(lower_expr(input, ctx)),
                stages: stages.iter().map(|stage| lower_expr(stage, ctx)).collect(),
            }
        }
        AstExprKind::Binary { op, left, right } => {
            ctx.features.insert(HirFeature::Binary);
            HirExprKind::Binary {
                op: *op,
                left: Box::new(lower_expr(left, ctx)),
                right: Box::new(lower_expr(right, ctx)),
            }
        }
        AstExprKind::Raw => {
            ctx.diagnostics.push(HirDiagnostic {
                span: expr.span,
                message: format!("expression kept as raw syntax: {}", expr.raw),
            });
            HirExprKind::Unsupported {
                reason: "raw syntax not yet lowered".to_string(),
            }
        }
    };
    HirExpr {
        kind,
        span: expr.span,
        raw: expr.raw.clone(),
    }
}

fn lower_call(
    path: &str,
    args: &[boon_syntax::AstCallArg],
    span: boon_syntax::Span,
    ctx: &mut LowerContext,
) -> HirExprKind {
    let lowered_args = args
        .iter()
        .map(|arg| HirCallArg {
            name: arg.name.clone(),
            value: lower_expr(&arg.value, ctx),
            span: arg.span,
        })
        .collect::<Vec<_>>();
    if path.starts_with("Element/") {
        ctx.features.insert(HirFeature::HostElement);
        HirExprKind::HostCall {
            path: path.to_string(),
            args: lowered_args,
        }
    } else if path.starts_with("Document/") {
        ctx.features.insert(HirFeature::HostDocument);
        HirExprKind::HostCall {
            path: path.to_string(),
            args: lowered_args,
        }
    } else if path.starts_with("List/") {
        let op = match path {
            "List/append" => {
                ctx.features.insert(HirFeature::ListAppend);
                HirListOp::Append
            }
            "List/remove" => {
                ctx.features.insert(HirFeature::ListRemove);
                HirListOp::Remove
            }
            "List/retain" => {
                ctx.features.insert(HirFeature::ListRetain);
                HirListOp::Retain
            }
            "List/map" => {
                ctx.features.insert(HirFeature::ListMap);
                HirListOp::Map
            }
            "List/count" => {
                ctx.features.insert(HirFeature::ListCount);
                HirListOp::Count
            }
            "List/range" => {
                ctx.features.insert(HirFeature::ListRange);
                HirListOp::Range
            }
            _ => {
                ctx.diagnostics.push(HirDiagnostic {
                    span,
                    message: format!("unsupported List operation `{path}`"),
                });
                HirListOp::Unknown {
                    path: path.to_string(),
                }
            }
        };
        HirExprKind::ListCall {
            op,
            args: lowered_args,
        }
    } else {
        if path.starts_with("Math/") {
            ctx.features.insert(HirFeature::MathCall);
            if !matches!(path, "Math/add" | "Math/sum") {
                ctx.diagnostics.push(HirDiagnostic {
                    span,
                    message: format!("unsupported Math operation `{path}`"),
                });
            }
        } else {
            ctx.features.insert(HirFeature::FunctionCall);
        }
        HirExprKind::FunctionCall {
            path: path.to_string(),
            args: lowered_args,
        }
    }
}

fn collect_record_dependency(key: &str, value: &HirExpr, ctx: &mut LowerContext) {
    let mut paths = Vec::new();
    collect_paths(value, &mut paths);
    for path in paths {
        ctx.dependencies.push(HirDependency {
            from: key.to_string(),
            to: path,
            span: value.span,
        });
    }
}

fn collect_paths(expr: &HirExpr, paths: &mut Vec<String>) {
    match &expr.kind {
        HirExprKind::Path { value } => paths.push(value.clone()),
        HirExprKind::Record { entries } => {
            for entry in entries {
                if let Some(value) = &entry.value {
                    collect_paths(value, paths);
                }
                for child in &entry.children {
                    if let Some(value) = &child.value {
                        collect_paths(value, paths);
                    }
                }
            }
        }
        HirExprKind::List { items } => {
            for item in items {
                collect_paths(item, paths);
            }
        }
        HirExprKind::Block { bindings } => {
            for binding in bindings {
                collect_paths(&binding.value, paths);
            }
        }
        HirExprKind::When { arms } | HirExprKind::While { arms } => {
            for arm in arms {
                collect_paths(&arm.value, paths);
            }
        }
        HirExprKind::Then { body } | HirExprKind::Hold { body, .. } => collect_paths(body, paths),
        HirExprKind::Latest { branches } => {
            for branch in branches {
                collect_paths(branch, paths);
            }
        }
        HirExprKind::HostCall { args, .. }
        | HirExprKind::ListCall { args, .. }
        | HirExprKind::FunctionCall { args, .. } => {
            for arg in args {
                collect_paths(&arg.value, paths);
            }
        }
        HirExprKind::Pipeline { input, stages } => {
            collect_paths(input, paths);
            for stage in stages {
                collect_paths(stage, paths);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_paths(left, paths);
            collect_paths(right, paths);
        }
        HirExprKind::Source
        | HirExprKind::Literal { .. }
        | HirExprKind::Tag { .. }
        | HirExprKind::Passed
        | HirExprKind::Skip
        | HirExprKind::Unsupported { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boon_syntax::parse_module;

    #[test]
    fn lower_exposes_core_language_features() {
        let source = r#"
store:
    sources:
        input:
            text: SOURCE
            event:
                key_down:
                    key: SOURCE

FUNCTION make_item(title) {
    [
        title: title
    ]
}

title_to_add:
    store.sources.input.event.key_down.key
    |> WHEN {
        Enter => BLOCK {
            trimmed: store.sources.input.text |> Text/trim()
            trimmed |> Text/is_not_empty() |> WHEN { True => trimmed False => SKIP }
        }
        __ => SKIP
    }

branch:
    title_to_add
    |> WHILE {
        __ => TEXT { fallback }
    }

items:
    LIST {
        make_item(title: TEXT { First })
    }
    |> List/append(item: title_to_add |> make_item(title: PASSED))
    |> List/remove(item, on: item.sources.remove_button.event.press)
"#;
        let hir = lower(parse_module("fixture", source).expect("fixture parses"));
        for feature in [
            HirFeature::Source,
            HirFeature::FunctionDefinition,
            HirFeature::Pipeline,
            HirFeature::When,
            HirFeature::While,
            HirFeature::Block,
            HirFeature::ListLiteral,
            HirFeature::ListAppend,
            HirFeature::ListRemove,
            HirFeature::FunctionCall,
        ] {
            assert!(
                hir.features.contains(&feature),
                "missing HIR feature {feature:?}: {:#?}",
                hir.features
            );
        }
    }
}
