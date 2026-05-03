use anyhow::{Result, bail};
use boon_hir::{HirModule, lower};
use boon_host_schema::{HostContract, element_contracts};
use boon_shape::Shape;
use boon_source::{SourceEntry, SourceInventory, SourceOwner};
use boon_syntax::{ParsedModule, ParsedRecordEntry, parse_module};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompiledModule {
    pub name: String,
    pub hir: HirModule,
    pub sources: SourceInventory,
    pub program: ProgramSpec,
    pub app_ir: AppIr,
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
    pub event_handlers: Vec<IrEventHandler>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrStateCell {
    pub path: String,
    pub initial: IrValueExpr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IrEventHandler {
    pub source_path: String,
    pub when: Option<IrPredicate>,
    pub effects: Vec<IrEffect>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrEffect {
    Assign {
        state_path: String,
        expr: IrValueExpr,
    },
    ListAppendText {
        list_path: String,
        text_state_path: String,
        trim: bool,
        skip_empty: bool,
    },
    ListToggleAllMarks {
        list_path: String,
    },
    ListToggleOwnerMark {
        list_path: String,
    },
    ListRemoveOwner {
        list_path: String,
    },
    ListRemoveMarked {
        list_path: String,
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
pub struct ProgramSpec {
    pub title: String,
    pub scene: SurfaceKind,
    pub scalar_accumulator: Option<AccumulatorSpec>,
    pub clock_accumulator: Option<ClockAccumulatorSpec>,
    pub sequence: Option<SequenceSpec>,
    pub dense_grid: Option<DenseGridSpec>,
    pub kinematics: Option<KinematicSpec>,
    pub physical_debug: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    #[default]
    Blank,
    ActionValue,
    ClockValue,
    Sequence,
    DenseGrid,
    Kinematics,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccumulatorSpec {
    pub event_path: String,
    pub state_path: String,
    pub initial: i64,
    pub step: i64,
    pub button_label: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClockAccumulatorSpec {
    pub event_path: String,
    pub state_path: String,
    pub quantum_ms: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SequenceSpec {
    pub initial_texts: Vec<String>,
    pub append_on_submit: bool,
    pub actions: SequenceActionsSpec,
    pub view_selectors: Vec<SequenceSelectorSpec>,
    pub view: SequenceViewSpec,
    pub dynamic_mark_toggle: bool,
    pub dynamic_remove: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SequenceActionsSpec {
    pub mass_mark_event_path: Option<String>,
    pub remove_marked_event_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SequenceSelectorSpec {
    pub id: String,
    pub event_path: String,
    pub label: String,
    pub visibility: RecordVisibility,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SequenceViewSpec {
    pub title_line: String,
    pub entry_hint: String,
    pub count_suffix: String,
    pub remove_marked_label: Option<String>,
    pub auxiliary_lines: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordVisibility {
    #[default]
    All,
    Unmarked,
    Marked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DenseGridSpec {
    pub rows: usize,
    pub columns: usize,
    pub editor_source_family: String,
    pub expression_functions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct KinematicSpec {
    pub frame_event_path: String,
    pub control_event_path: String,
    pub arena_width: i64,
    pub arena_height: i64,
    pub body: MovingBodySpec,
    pub primary_control: ControllerSpec,
    pub tracked_control: Option<ControllerSpec>,
    pub contact_field: Option<ContactFieldSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MovingBodySpec {
    pub x: i64,
    pub y: i64,
    pub dx: i64,
    pub dy: i64,
    pub size: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ControllerSpec {
    pub axis: ControlAxis,
    pub position: i64,
    pub step: i64,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub auto_track: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContactFieldSpec {
    pub rows: usize,
    pub columns: usize,
    pub top: i64,
    pub margin: i64,
    pub gap: i64,
    pub height: i64,
    pub value_per_contact: i64,
}

pub fn compile_source(name: &str, source: &str) -> Result<CompiledModule> {
    let parsed = parse_module(name, source)?;
    let hir = lower(parsed.clone());
    let provenance = compiled_provenance(source, &hir)?;
    let dynamic_sequence_root = infer_dynamic_sequence_root(&parsed);
    let dynamic_dense_root = infer_dense_source_root(&parsed);
    let map_alias_roots = map_alias_roots(
        &parsed,
        dynamic_sequence_root.as_deref(),
        dynamic_dense_root.as_deref(),
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
    let program = program_spec(name, &parsed, &sources);
    let app_ir = app_ir_from_program(&program, &sources);
    Ok(CompiledModule {
        name: name.to_string(),
        hir,
        sources,
        program,
        app_ir,
        provenance,
    })
}

fn app_ir_from_program(program: &ProgramSpec, sources: &SourceInventory) -> AppIr {
    let mut ir = AppIr::default();
    if let Some(accumulator) = &program.scalar_accumulator {
        push_accumulator_ir(
            &mut ir,
            &accumulator.event_path,
            &accumulator.state_path,
            accumulator.initial,
            accumulator.step,
        );
    }
    if let Some(accumulator) = &program.clock_accumulator {
        push_accumulator_ir(
            &mut ir,
            &accumulator.event_path,
            &accumulator.state_path,
            0,
            1,
        );
    }
    if let Some(sequence) = &program.sequence {
        let dynamic_family = dynamic_source_families(sources).first().cloned();
        let list_path = dynamic_family
            .as_deref()
            .map(dynamic_family_root)
            .unwrap_or_else(|| "records".to_string());
        if sequence.append_on_submit
            && let Some(static_text_path) =
                static_source_with_producer(sources, "Element/text_input(element.text)")
            && let Some(static_key_path) =
                source_base_from_path(&static_text_path).and_then(|base| {
                    existing_source_path(sources, &format!("{base}.event.key_down.key"))
                })
        {
            ir.event_handlers.push(IrEventHandler {
                source_path: static_key_path.clone(),
                when: Some(IrPredicate::SourceTagEquals {
                    path: static_key_path,
                    tag: "Enter".to_string(),
                }),
                effects: vec![
                    IrEffect::ListAppendText {
                        list_path: list_path.clone(),
                        text_state_path: static_text_path.clone(),
                        trim: true,
                        skip_empty: true,
                    },
                    IrEffect::ClearText {
                        text_state_path: static_text_path,
                    },
                ],
            });
        }
        if let Some(event_path) = &sequence.actions.mass_mark_event_path {
            ir.event_handlers.push(IrEventHandler {
                source_path: event_path.clone(),
                when: None,
                effects: vec![IrEffect::ListToggleAllMarks {
                    list_path: list_path.clone(),
                }],
            });
        }
        if let Some(event_path) = &sequence.actions.remove_marked_event_path {
            ir.event_handlers.push(IrEventHandler {
                source_path: event_path.clone(),
                when: None,
                effects: vec![IrEffect::ListRemoveMarked {
                    list_path: list_path.clone(),
                }],
            });
        }
        for selector in &sequence.view_selectors {
            ir.event_handlers.push(IrEventHandler {
                source_path: selector.event_path.clone(),
                when: None,
                effects: vec![IrEffect::SetTagState {
                    state_path: "view_selector".to_string(),
                    value: selector.id.clone(),
                }],
            });
        }
        if let Some(dynamic_family) = dynamic_family.as_deref() {
            if sequence.dynamic_mark_toggle
                && let Some(event_path) = source_family_with_producer(
                    sources,
                    dynamic_family,
                    "Element/checkbox(element.event.click)",
                )
            {
                ir.event_handlers.push(IrEventHandler {
                    source_path: event_path,
                    when: None,
                    effects: vec![IrEffect::ListToggleOwnerMark {
                        list_path: list_path.clone(),
                    }],
                });
            }
            if sequence.dynamic_remove
                && let Some(event_path) = source_family_with_producer(
                    sources,
                    dynamic_family,
                    "Element/button(element.event.press)",
                )
            {
                ir.event_handlers.push(IrEventHandler {
                    source_path: event_path,
                    when: None,
                    effects: vec![IrEffect::ListRemoveOwner {
                        list_path: list_path.clone(),
                    }],
                });
            }
        }
    }
    ir
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

fn source_base_from_path(path: &str) -> Option<String> {
    for suffix in [
        ".text",
        ".checked",
        ".hovered",
        ".event.press",
        ".event.click",
        ".event.change",
        ".event.blur",
        ".event.focus",
        ".event.double_click",
        ".event.key_down.key",
        ".event.tick",
        ".event.frame",
    ] {
        if let Some(base) = path.strip_suffix(suffix) {
            return Some(base.to_string());
        }
    }
    None
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
    dynamic_dense_root: Option<&str>,
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
                    .zip(dynamic_dense_root)
                    .and_then(|(collection, root)| (collection == root).then(|| root.to_string()))
            })
            .or_else(|| {
                (dynamic_sequence_root.is_some() && dynamic_dense_root.is_none())
                    .then(|| dynamic_sequence_root.expect("checked").to_string())
            })
            .or_else(|| {
                (dynamic_dense_root.is_some() && dynamic_sequence_root.is_none())
                    .then(|| dynamic_dense_root.expect("checked").to_string())
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

fn program_spec(name: &str, parsed: &ParsedModule, sources: &SourceInventory) -> ProgramSpec {
    let title = first_child_text(parsed, "title").unwrap_or_else(|| name.replace('_', " "));
    let has_dense_element = parsed
        .module_calls
        .iter()
        .any(|call| call.path == "Element/grid");
    let dynamic_families = dynamic_source_families(sources);
    let sequence_family = (!has_dense_element)
        .then(|| dynamic_families.first().cloned())
        .flatten();
    let dense_family = has_dense_element
        .then(|| dynamic_families.first().cloned())
        .flatten();

    let view_selectors = sequence_view_selectors(parsed, sources);
    let sequence = sequence_family.as_ref().map(|family| SequenceSpec {
        initial_texts: initial_sequence_literals(parsed, family),
        append_on_submit: module_called(parsed, "List/append")
            && static_source_with_producer(sources, "Element/text_input(element.text)").is_some(),
        actions: sequence_actions(parsed, sources),
        view_selectors,
        view: sequence_view_spec(parsed, &title),
        dynamic_mark_toggle: source_family_with_producer(
            sources,
            family,
            "Element/checkbox(element.event.click)",
        )
        .is_some(),
        dynamic_remove: source_family_with_producer(
            sources,
            family,
            "Element/button(element.event.press)",
        )
        .is_some(),
    });
    let dense_grid = dense_family.as_ref().map(|family| DenseGridSpec {
        rows: range_to(parsed, "rows").unwrap_or(100),
        columns: range_to(parsed, "columns").unwrap_or(26),
        editor_source_family: source_family_with_producer(
            sources,
            family,
            "Element/text_input(element.text)",
        )
        .and_then(|path| path.strip_suffix(".text").map(str::to_string))
        .unwrap_or_else(|| format!("{family}.sources.editor")),
        expression_functions: expression_functions(parsed),
    });
    let scalar_accumulator =
        static_source_with_producer(sources, "Element/button(element.event.press)")
            .filter(|_| {
                sequence.is_none()
                    && dense_grid.is_none()
                    && top_record(parsed, "kinematics").is_none()
            })
            .map(|event_path| AccumulatorSpec {
                event_path,
                state_path: "scalar_value".to_string(),
                initial: 0,
                step: first_hold_step(parsed).unwrap_or(0),
                button_label: scalar_button_label(parsed).unwrap_or_default(),
            });
    let clock_accumulator = static_tick_source(sources)
        .filter(|_| scalar_accumulator.is_none() && top_record(parsed, "kinematics").is_none())
        .map(|event_path| ClockAccumulatorSpec {
            event_path,
            state_path: "clock_value".to_string(),
            quantum_ms: 1000,
        });
    let kinematics = top_record(parsed, "kinematics").map(|record| kinematic_spec(record, sources));
    let scene = if sequence.is_some() {
        SurfaceKind::Sequence
    } else if dense_grid.is_some() {
        SurfaceKind::DenseGrid
    } else if scalar_accumulator.is_some() {
        SurfaceKind::ActionValue
    } else if clock_accumulator.is_some() {
        SurfaceKind::ClockValue
    } else if kinematics.is_some() {
        SurfaceKind::Kinematics
    } else {
        SurfaceKind::Blank
    };
    ProgramSpec {
        title,
        scene,
        scalar_accumulator,
        clock_accumulator,
        sequence,
        dense_grid,
        kinematics,
        physical_debug: record_bool(top_record(parsed, "view"), "physical_debug").unwrap_or(false),
    }
}

fn kinematic_spec(record: &ParsedRecordEntry, sources: &SourceInventory) -> KinematicSpec {
    let arena = child_record(record, "arena");
    let body = child_record(record, "body");
    let primary_control = child_record(record, "primary_control");
    let tracked_control = child_record(record, "tracked_control");
    let contact_field = child_record(record, "contact_field");
    KinematicSpec {
        frame_event_path: first_static_path_matching(sources, ".event.frame").unwrap_or_default(),
        control_event_path: first_static_path_matching(sources, ".event.key_down.key")
            .unwrap_or_default(),
        arena_width: record_number(arena, "width").unwrap_or(1000),
        arena_height: record_number(arena, "height").unwrap_or(700),
        body: MovingBodySpec {
            x: record_number(body, "x").unwrap_or(500),
            y: record_number(body, "y").unwrap_or(350),
            dx: record_number(body, "dx").unwrap_or(10),
            dy: record_number(body, "dy").unwrap_or(8),
            size: record_number(body, "size").unwrap_or(22),
        },
        primary_control: controller_spec(
            primary_control,
            ControlAxis::Vertical,
            ControllerSpec {
                axis: ControlAxis::Vertical,
                position: 50,
                step: 8,
                x: 38,
                y: 0,
                width: 18,
                height: 128,
                auto_track: false,
            },
        ),
        tracked_control: tracked_control.map(|block| {
            controller_spec(
                Some(block),
                ControlAxis::Vertical,
                ControllerSpec {
                    axis: ControlAxis::Vertical,
                    position: 50,
                    step: 8,
                    x: 944,
                    y: 0,
                    width: 18,
                    height: 128,
                    auto_track: true,
                },
            )
        }),
        contact_field: contact_field.map(|block| ContactFieldSpec {
            rows: record_number(Some(block), "rows").unwrap_or(6).max(0) as usize,
            columns: record_number(Some(block), "columns").unwrap_or(12).max(0) as usize,
            top: record_number(Some(block), "top").unwrap_or(56),
            margin: record_number(Some(block), "margin").unwrap_or(36),
            gap: record_number(Some(block), "gap").unwrap_or(8),
            height: record_number(Some(block), "height").unwrap_or(28),
            value_per_contact: record_number(Some(block), "value_per_contact").unwrap_or(10),
        }),
    }
}

fn controller_spec(
    block: Option<&ParsedRecordEntry>,
    default_axis: ControlAxis,
    default: ControllerSpec,
) -> ControllerSpec {
    ControllerSpec {
        axis: record_axis(block, "axis").unwrap_or(default_axis),
        position: record_number(block, "position").unwrap_or(default.position),
        step: record_number(block, "step").unwrap_or(default.step),
        x: record_number(block, "x").unwrap_or(default.x),
        y: record_number(block, "y").unwrap_or(default.y),
        width: record_number(block, "width").unwrap_or(default.width),
        height: record_number(block, "height").unwrap_or(default.height),
        auto_track: record_bool(block, "auto_track").unwrap_or(default.auto_track),
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

fn static_source_with_producer(sources: &SourceInventory, producer: &str) -> Option<String> {
    sources
        .entries
        .iter()
        .find(|entry| matches!(&entry.owner, SourceOwner::Static) && entry.producer == producer)
        .map(|entry| entry.path.clone())
}

fn static_paths_for_producer(sources: &SourceInventory, producer: &str) -> Vec<String> {
    sources
        .entries
        .iter()
        .filter(|entry| matches!(&entry.owner, SourceOwner::Static) && entry.producer == producer)
        .map(|entry| entry.path.clone())
        .collect()
}

fn source_family_with_producer(
    sources: &SourceInventory,
    family: &str,
    producer: &str,
) -> Option<String> {
    sources
        .entries
        .iter()
        .find(|entry| entry.path.starts_with(family) && entry.producer == producer)
        .map(|entry| entry.path.clone())
}

fn first_static_path_matching(sources: &SourceInventory, suffix: &str) -> Option<String> {
    sources
        .entries
        .iter()
        .find(|entry| matches!(&entry.owner, SourceOwner::Static) && entry.path.ends_with(suffix))
        .map(|entry| entry.path.clone())
}

fn static_tick_source(sources: &SourceInventory) -> Option<String> {
    first_static_path_matching(sources, ".event.tick")
}

fn sequence_view_selectors(
    parsed: &ParsedModule,
    sources: &SourceInventory,
) -> Vec<SequenceSelectorSpec> {
    top_record(parsed, "view")
        .and_then(|view| child_record(view, "selectors"))
        .map(|selectors| {
            selectors
                .children
                .iter()
                .filter_map(|selector| {
                    let event_path = static_source_event_by_base_name(
                        sources,
                        &selector.key,
                        "Element/button(element.event.press)",
                    )?;
                    let label = child_text(parsed, selector, "label")
                        .unwrap_or_else(|| selector.key.clone());
                    let visibility =
                        record_visibility(child_record(selector, "visibility")).unwrap_or_default();
                    Some(SequenceSelectorSpec {
                        id: selector.key.clone(),
                        event_path,
                        label,
                        visibility,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn sequence_actions(parsed: &ParsedModule, sources: &SourceInventory) -> SequenceActionsSpec {
    let action_record = top_record(parsed, "view").and_then(|view| child_record(view, "actions"));
    let mass_mark_event_path = action_record
        .and_then(|actions| child_record(actions, "mass_mark"))
        .and_then(|action| action_source_name(action))
        .and_then(|source_name| {
            static_source_event_by_base_name(
                sources,
                source_name,
                "Element/checkbox(element.event.click)",
            )
        });
    let remove_marked_event_path = action_record
        .and_then(|actions| child_record(actions, "remove_marked"))
        .and_then(|action| action_source_name(action))
        .and_then(|source_name| {
            static_source_event_by_base_name(
                sources,
                source_name,
                "Element/button(element.event.press)",
            )
        });
    SequenceActionsSpec {
        mass_mark_event_path,
        remove_marked_event_path,
    }
}

fn action_source_name(action: &ParsedRecordEntry) -> Option<&str> {
    child_record(action, "source")?
        .value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn static_source_event_by_base_name(
    sources: &SourceInventory,
    source_name: &str,
    producer: &str,
) -> Option<String> {
    static_paths_for_producer(sources, producer)
        .into_iter()
        .find(|path| {
            path.strip_suffix(source_event_suffix(producer))
                .and_then(|base| base.rsplit('.').next())
                == Some(source_name)
        })
}

fn source_event_suffix(producer: &str) -> &'static str {
    match producer {
        "Element/checkbox(element.event.click)" => ".event.click",
        "Element/button(element.event.press)" => ".event.press",
        _ => "",
    }
}

fn sequence_view_spec(parsed: &ParsedModule, title: &str) -> SequenceViewSpec {
    let view = top_record(parsed, "view");
    let auxiliary_lines = view
        .and_then(|view| child_record(view, "auxiliary"))
        .map(|auxiliary| {
            auxiliary
                .children
                .iter()
                .filter_map(|entry| text_literal_on_line(parsed, entry.span.line))
                .filter(|text| !text.is_empty())
                .collect()
        })
        .unwrap_or_default();
    SequenceViewSpec {
        title_line: view
            .and_then(|view| child_text(parsed, view, "title_line"))
            .unwrap_or_else(|| title.to_string()),
        entry_hint: view
            .and_then(|view| child_text(parsed, view, "entry_hint"))
            .unwrap_or_default(),
        count_suffix: view
            .and_then(|view| child_text(parsed, view, "count_suffix"))
            .unwrap_or_else(|| "unmarked".to_string()),
        remove_marked_label: view
            .and_then(|view| child_record(view, "actions"))
            .and_then(|actions| child_record(actions, "remove_marked"))
            .and_then(|action| child_text(parsed, action, "label")),
        auxiliary_lines,
    }
}

fn record_visibility(record: Option<&ParsedRecordEntry>) -> Option<RecordVisibility> {
    match record?.value.as_deref()?.trim() {
        "All" => Some(RecordVisibility::All),
        "Unmarked" => Some(RecordVisibility::Unmarked),
        "Marked" => Some(RecordVisibility::Marked),
        _ => None,
    }
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

fn infer_dense_source_root(parsed: &ParsedModule) -> Option<String> {
    module_called(parsed, "Element/grid").then_some(())?;
    parsed
        .records
        .iter()
        .find(|record| record.key != "store" && child_record(record, "sources").is_some())
        .map(|record| record.key.clone())
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

fn initial_sequence_literals(parsed: &ParsedModule, binding: &str) -> Vec<String> {
    let root = binding.strip_suffix("[*]").unwrap_or(binding);
    let Some(record) = top_record(parsed, root) else {
        return Vec::new();
    };
    parsed
        .text_literals
        .iter()
        .filter(|literal| span_under_record(parsed, literal.span.line, record))
        .map(|literal| literal.value.clone())
        .collect()
}

fn range_to(parsed: &ParsedModule, binding: &str) -> Option<usize> {
    let record = top_record(parsed, binding)?;
    parsed
        .module_calls
        .iter()
        .find(|call| span_under_record(parsed, call.span.line, record) && call.path == "List/range")
        .and_then(|call| call_arg(call, "to"))
        .and_then(|value| value.parse().ok())
}

fn call_arg<'a>(call: &'a boon_syntax::ModuleCall, name: &str) -> Option<&'a str> {
    call.args
        .iter()
        .find(|arg| arg.name == name)
        .map(|arg| arg.value.as_str())
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

fn first_hold_step(parsed: &ParsedModule) -> Option<i64> {
    let record = parsed.records.iter().find(|record| {
        !matches!(
            record.key.as_str(),
            "store" | "view" | "document" | "kinematics"
        ) && parsed
            .state_steps
            .iter()
            .any(|step| step.state == "state" && span_under_record(parsed, step.span.line, record))
    })?;
    parsed
        .state_steps
        .iter()
        .find(|step| step.state == "state" && span_under_record(parsed, step.span.line, record))
        .map(|step| step.amount)
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

fn record_number(record: Option<&ParsedRecordEntry>, field: &str) -> Option<i64> {
    child_record(record?, field)?
        .value
        .as_deref()?
        .trim()
        .parse()
        .ok()
}

fn record_axis(record: Option<&ParsedRecordEntry>, field: &str) -> Option<ControlAxis> {
    match child_record(record?, field)?.value.as_deref()?.trim() {
        "Horizontal" => Some(ControlAxis::Horizontal),
        "Vertical" => Some(ControlAxis::Vertical),
        _ => None,
    }
}

fn record_bool(record: Option<&ParsedRecordEntry>, field: &str) -> Option<bool> {
    match child_record(record?, field)?.value.as_deref()?.trim() {
        "True" => Some(true),
        "False" => Some(false),
        _ => None,
    }
}

fn expression_functions(parsed: &ParsedModule) -> Vec<String> {
    parsed
        .records
        .iter()
        .filter_map(|record| child_record(record, "functions"))
        .flat_map(|functions| functions.children.iter())
        .filter(|entry| {
            entry
                .value
                .as_deref()
                .is_some_and(|value| value.trim().starts_with("Math/"))
        })
        .map(|entry| entry.key.clone())
        .collect()
}

fn module_called(parsed: &ParsedModule, path: &str) -> bool {
    parsed.module_calls.iter().any(|call| call.path == path)
}
