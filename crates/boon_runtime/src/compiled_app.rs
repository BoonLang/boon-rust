use crate::{
    AppSnapshot, BoonApp, RuntimeClock, SourceBatch, SourceEmission, SourceInventory, SourceValue,
    StateDelta, TurnId, TurnMetrics, TurnResult,
};
use anyhow::{Context, Result, bail};
use boon_compiler::{
    AppIr, ExecEffect, ExecExpr, ExecutableIr, IrAppMetadata, IrCollectionPredicate,
    IrCollectionValueExpr, IrEffect, IrPredicate, IrStaticField, IrStaticRecord, IrStaticValue,
    IrValueExpr,
};
use boon_render_ir::{
    DrawCommand, FrameScene, HitTarget, HitTargetAction, HostPatch, NodeId, NodeKind,
};
use boon_shape::Shape;
use boon_source::{SourceEntry, SourceOwner};
use boon_stdlib::FormulaBook;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct CompiledApp {
    program: IrAppMetadata,
    app_ir: AppIr,
    executable_ir: ExecutableIr,
    inventory: SourceInventory,
    wiring: RuntimeWiring,
    turn: u64,
    frame_text: String,
    clock: RuntimeClock,
    dynamic_values: Vec<RuntimeDynamicValue>,
    next_dynamic_value_id: u64,
    entry_text: String,
    source_state: BTreeMap<String, SourceValue>,
    generic_state: BTreeMap<String, i64>,
    view_selector: String,
    grid: FormulaBook,
    grid_selected: (usize, usize),
    grid_edit_focus: Option<(usize, usize)>,
}

#[derive(Clone, Debug, Default)]
struct RuntimeWiring {
    action_state: Option<StateEventBinding>,
    clock_state: Option<StateEventBinding>,
    list: Option<CollectionSourceWiring>,
    grid: Option<ElementGridWiring>,
}

#[derive(Clone, Debug)]
struct StateEventBinding {
    state_path: String,
}

#[derive(Clone, Debug)]
struct CollectionSourceWiring {
    family: String,
    root: String,
    text_field: String,
    bool_field: Option<String>,
    edit_focus_field: Option<String>,
    entry_text: Option<String>,
    input_focus: Option<String>,
    input_blur: Option<String>,
    input_change: Option<String>,
    dynamic_text_value: Option<String>,
    dynamic_text_key: Option<String>,
    dynamic_text_blur: Option<String>,
    dynamic_text_change: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct CollectionView {
    selectors: Vec<CollectionViewSelector>,
}

#[derive(Clone, Debug)]
struct CollectionViewSelector {
    id: String,
    visibility: CollectionViewVisibility,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum CollectionViewVisibility {
    #[default]
    All,
    Unmarked,
    Marked,
}

#[derive(Clone, Debug)]
struct ElementGridWiring {
    family: String,
    root: String,
    display_double_click: Option<String>,
    editor_text: Option<String>,
    editor_key: Option<String>,
    viewport_key: Option<String>,
}

impl RuntimeWiring {
    fn from_compiled(
        app_ir: &AppIr,
        executable_ir: &ExecutableIr,
        inventory: &SourceInventory,
    ) -> Self {
        let mut action_state = None;
        let mut clock_state = None;
        for handler in &executable_ir.source_handlers {
            let Some(state_path) = handler.effects.first().map(|effect| match effect {
                ExecEffect::SetState { path, .. } => path.clone(),
            }) else {
                continue;
            };
            let binding = StateEventBinding { state_path };
            if source_shape_is_tick(inventory, &handler.source_path) {
                clock_state.get_or_insert(binding);
            } else {
                action_state.get_or_insert(binding);
            }
        }
        for handler in &app_ir.event_handlers {
            let Some(state_path) = handler.effects.iter().find_map(|effect| match effect {
                IrEffect::Assign { state_path, .. } => Some(state_path.clone()),
                _ => None,
            }) else {
                continue;
            };
            let binding = StateEventBinding { state_path };
            if source_shape_is_tick(inventory, &handler.source_path) {
                clock_state.get_or_insert(binding);
            } else {
                action_state.get_or_insert(binding);
            }
        }
        let list = CollectionSourceWiring::from_app_ir(inventory, app_ir);
        let grid = app_ir
            .render_tree
            .as_ref()
            .is_some_and(|tree| render_node_has_kind(tree, &boon_compiler::IrRenderKind::Grid))
            .then(|| ElementGridWiring::from_inventory(inventory))
            .flatten();
        Self {
            action_state,
            clock_state,
            list,
            grid,
        }
    }
}

impl CollectionSourceWiring {
    fn from_app_ir(inventory: &SourceInventory, app_ir: &AppIr) -> Option<Self> {
        if !app_ir.event_handlers.iter().any(|handler| {
            handler.effects.iter().any(|effect| {
                matches!(
                    effect,
                    IrEffect::CollectionAppendRecord { .. }
                        | IrEffect::CollectionUpdateAllFields { .. }
                        | IrEffect::CollectionUpdateOwnerField { .. }
                        | IrEffect::CollectionRemoveCurrent { .. }
                        | IrEffect::CollectionRemoveWhere { .. }
                )
            })
        }) {
            return None;
        }
        let dynamic_family =
            first_dynamic_family(inventory, "Element/checkbox(element.event.click)")
                .or_else(|| first_dynamic_family(inventory, "Element/text_input(element.text)"));
        let root = dynamic_family
            .as_deref()
            .map(dynamic_family_root)
            .or_else(|| first_collection_effect_path(app_ir))
            .or_else(|| {
                app_ir
                    .collection_states
                    .first()
                    .map(|list| list.path.clone())
            })?;
        let entry_text = app_ir.event_handlers.iter().find_map(|handler| {
            handler.effects.iter().find_map(|effect| match effect {
                IrEffect::CollectionAppendRecord { fields, .. } => {
                    fields.iter().find_map(|field| match &field.value {
                        IrCollectionValueExpr::SourceText { path, .. } => Some(path.clone()),
                        _ => None,
                    })
                }
                _ => None,
            })
        });
        let text_field = app_ir
            .event_handlers
            .iter()
            .find_map(|handler| {
                handler.effects.iter().find_map(|effect| match effect {
                    IrEffect::CollectionAppendRecord { fields, .. } => {
                        fields.iter().find_map(|field| {
                            matches!(field.value, IrCollectionValueExpr::SourceText { .. })
                                .then(|| field.field.clone())
                        })
                    }
                    _ => None,
                })
            })
            .or_else(|| {
                app_ir
                    .collection_states
                    .first()
                    .and_then(|collection| first_text_field(&collection.initial_entries))
            })
            .unwrap_or_else(|| "value".to_string());
        let bool_field = app_ir.event_handlers.iter().find_map(|handler| {
            handler.effects.iter().find_map(|effect| match effect {
                IrEffect::CollectionUpdateAllFields { field, .. }
                | IrEffect::CollectionUpdateOwnerField { field, .. } => Some(field.clone()),
                IrEffect::CollectionRemoveWhere {
                    predicate: IrCollectionPredicate::FieldBoolEquals { field, .. },
                    ..
                } => Some(field.clone()),
                _ => None,
            })
        });
        let input_base = entry_text
            .as_deref()
            .and_then(source_base_from_path)
            .or_else(|| static_base_for_producer(inventory, "Element/text_input(element.text)"));
        let dynamic_text_base = dynamic_family.as_deref().and_then(|family| {
            dynamic_base_for_producer(inventory, family, "Element/text_input(element.text)")
        });
        let family = dynamic_family.unwrap_or_else(|| format!("{root}[*]"));
        let edit_focus_field = dynamic_text_base.as_ref().and_then(|base| {
            base.strip_prefix(&format!("{family}."))
                .map(str::to_string)
                .or_else(|| source_base_from_path(base))
        });
        Some(Self {
            family: family.clone(),
            root,
            text_field,
            bool_field,
            edit_focus_field,
            entry_text: entry_text
                .or_else(|| input_base.as_ref().map(|base| format!("{base}.text"))),
            input_focus: input_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.focus"))),
            input_blur: input_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.blur"))),
            input_change: input_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.change"))),
            dynamic_text_value: dynamic_text_base
                .as_ref()
                .map(|base| format!("{base}.text")),
            dynamic_text_key: dynamic_text_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.key_down.key"))),
            dynamic_text_blur: dynamic_text_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.blur"))),
            dynamic_text_change: dynamic_text_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.change"))),
        })
    }

    fn bool_field(&self) -> Option<&str> {
        self.bool_field.as_deref()
    }

    fn edit_focus_field(&self) -> Option<&str> {
        self.edit_focus_field.as_deref()
    }
}

impl ElementGridWiring {
    fn from_inventory(inventory: &SourceInventory) -> Option<Self> {
        let family =
            first_dynamic_family(inventory, "Element/text_input(element.text)").or_else(|| {
                first_dynamic_family(inventory, "Element/label(element.event.double_click)")
            })?;
        let root = dynamic_family_root(&family);
        let editor_base =
            dynamic_base_for_producer(inventory, &family, "Element/text_input(element.text)");
        Some(Self {
            family: family.clone(),
            root,
            display_double_click: dynamic_path_for_producer(
                inventory,
                &family,
                "Element/label(element.event.double_click)",
            ),
            editor_text: editor_base.as_ref().map(|base| format!("{base}.text")),
            editor_key: editor_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.key_down.key"))),
            viewport_key: inventory
                .entries
                .iter()
                .find(|entry| entry.path.ends_with(".event.key_down.key") && is_static(entry))
                .map(|entry| entry.path.clone()),
        })
    }
}

fn is_static(entry: &SourceEntry) -> bool {
    matches!(&entry.owner, SourceOwner::Static)
}

fn first_static_source_path_matching(inventory: &SourceInventory, suffix: &str) -> Option<String> {
    inventory
        .entries
        .iter()
        .find(|entry| is_static(entry) && entry.path.ends_with(suffix))
        .map(|entry| entry.path.clone())
}

fn source_shape_is_tick(inventory: &SourceInventory, path: &str) -> bool {
    inventory
        .entries
        .iter()
        .any(|entry| entry.path == path && entry.path.ends_with(".event.tick"))
}

fn existing_path(inventory: &SourceInventory, path: &str) -> Option<String> {
    inventory
        .entries
        .iter()
        .any(|entry| entry.path == path)
        .then(|| path.to_string())
}

fn static_paths_for_producer(inventory: &SourceInventory, producer: &str) -> Vec<String> {
    inventory
        .entries
        .iter()
        .filter(|entry| is_static(entry) && entry.producer == producer)
        .map(|entry| entry.path.clone())
        .collect()
}

fn static_base_for_producer(inventory: &SourceInventory, producer: &str) -> Option<String> {
    static_paths_for_producer(inventory, producer)
        .into_iter()
        .next()
        .and_then(|path| source_base_from_path(&path))
}

fn selfless_eval_exec_number(expr: &ExecExpr) -> Result<i64> {
    match expr {
        ExecExpr::Number { value } => Ok(*value),
        _ => bail!("executable state slot initial value must be a number"),
    }
}

fn unique_paths(paths: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn first_collection_effect_path(app_ir: &AppIr) -> Option<String> {
    app_ir.event_handlers.iter().find_map(|handler| {
        handler.effects.iter().find_map(|effect| match effect {
            IrEffect::CollectionAppendRecord {
                collection_path, ..
            }
            | IrEffect::CollectionUpdateAllFields {
                collection_path, ..
            }
            | IrEffect::CollectionUpdateOwnerField {
                collection_path, ..
            }
            | IrEffect::CollectionRemoveCurrent { collection_path }
            | IrEffect::CollectionRemoveWhere {
                collection_path, ..
            } => Some(collection_path.clone()),
            _ => None,
        })
    })
}

fn first_dynamic_family(inventory: &SourceInventory, producer: &str) -> Option<String> {
    inventory
        .entries
        .iter()
        .find(|entry| !is_static(entry) && entry.producer == producer)
        .and_then(|entry| {
            entry
                .path
                .split_once(".sources.")
                .map(|(family, _)| family.to_string())
        })
}

fn dynamic_path_for_producer(
    inventory: &SourceInventory,
    family: &str,
    producer: &str,
) -> Option<String> {
    inventory
        .entries
        .iter()
        .find(|entry| entry.path.starts_with(family) && entry.producer == producer)
        .map(|entry| entry.path.clone())
}

fn dynamic_base_for_producer(
    inventory: &SourceInventory,
    family: &str,
    producer: &str,
) -> Option<String> {
    dynamic_path_for_producer(inventory, family, producer)
        .and_then(|path| source_base_from_path(&path))
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

fn generic_source_label(path: &str) -> String {
    path.rsplit('.')
        .find(|segment| {
            !matches!(
                *segment,
                "sources"
                    | "event"
                    | "press"
                    | "click"
                    | "text"
                    | "key_down"
                    | "key"
                    | "change"
                    | "focus"
                    | "blur"
            )
        })
        .unwrap_or(path)
        .replace('_', " ")
}

fn collection_view_from_app_ir(app_ir: &AppIr) -> Option<CollectionView> {
    let view = app_ir
        .static_records
        .iter()
        .find(|record| record.path == "view")?;
    Some(CollectionView {
        selectors: static_record_field(view, "selectors")
            .map(|selectors| {
                selectors
                    .iter()
                    .map(|field| CollectionViewSelector {
                        id: field.key.clone(),
                        visibility: static_value_field(&field.value, "visibility")
                            .and_then(static_value_tag)
                            .and_then(collection_view_visibility_from_tag)
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn first_text_field(seeds: &[boon_compiler::IrCollectionSeed]) -> Option<String> {
    seeds.iter().find_map(|seed| {
        seed.fields.iter().find_map(|field| {
            matches!(field.value, IrStaticValue::Text { .. }).then(|| field.key.clone())
        })
    })
}

fn collect_static_text_values(fields: &[IrStaticField], lines: &mut Vec<String>) {
    for field in fields {
        match &field.value {
            IrStaticValue::Text { value } if !value.is_empty() => lines.push(value.clone()),
            IrStaticValue::Record { fields } => collect_static_text_values(fields, lines),
            IrStaticValue::List { items } => {
                for item in items {
                    collect_static_value_text(item, lines);
                }
            }
            _ => {}
        }
    }
}

fn collect_static_value_text(value: &IrStaticValue, lines: &mut Vec<String>) {
    match value {
        IrStaticValue::Text { value } if !value.is_empty() => lines.push(value.clone()),
        IrStaticValue::Record { fields } => collect_static_text_values(fields, lines),
        IrStaticValue::List { items } => {
            for item in items {
                collect_static_value_text(item, lines);
            }
        }
        _ => {}
    }
}

fn static_record_field<'a>(record: &'a IrStaticRecord, field: &str) -> Option<&'a [IrStaticField]> {
    static_value_record(static_record_value_field(&record.fields, field)?)
}

fn static_record_value_field<'a>(
    record: &'a [IrStaticField],
    field: &str,
) -> Option<&'a IrStaticValue> {
    record
        .iter()
        .find(|candidate| candidate.key == field)
        .map(|candidate| &candidate.value)
}

fn static_value_field<'a>(value: &'a IrStaticValue, field: &str) -> Option<&'a IrStaticValue> {
    static_record_value_field(static_value_record(value)?, field)
}

fn static_value_record(value: &IrStaticValue) -> Option<&[IrStaticField]> {
    match value {
        IrStaticValue::Record { fields } => Some(fields),
        _ => None,
    }
}

fn static_value_tag(value: &IrStaticValue) -> Option<&str> {
    match value {
        IrStaticValue::Tag { value } => Some(value.as_str()),
        _ => None,
    }
}

fn collection_view_visibility_from_tag(value: &str) -> Option<CollectionViewVisibility> {
    match value {
        "All" => Some(CollectionViewVisibility::All),
        "Unmarked" => Some(CollectionViewVisibility::Unmarked),
        "Marked" => Some(CollectionViewVisibility::Marked),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeDynamicValue {
    id: u64,
    generation: u32,
    fields: BTreeMap<String, Value>,
    focus: BTreeMap<String, bool>,
}

impl RuntimeDynamicValue {
    fn new(id: u64, fields: BTreeMap<String, Value>) -> Self {
        Self {
            id,
            generation: 0,
            fields,
            focus: BTreeMap::new(),
        }
    }

    fn from_literal_fields(id: u64, fields: &[IrStaticField]) -> Self {
        Self::new(
            id,
            fields
                .iter()
                .map(|field| (field.key.clone(), literal_value_to_json(&field.value)))
                .collect(),
        )
    }

    fn text_field(&self, path: &str) -> String {
        self.fields
            .get(path)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn set_text_field(&mut self, path: &str, value: String) {
        self.fields.insert(path.to_string(), json!(value));
    }

    fn bool_field(&self, path: &str) -> bool {
        self.fields
            .get(path)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    fn set_field_json(&mut self, path: &str, value: Value) {
        self.fields.insert(path.to_string(), value);
    }

    fn focus_field(&self, path: &str) -> bool {
        self.focus.get(path).copied().unwrap_or(false)
    }

    fn set_focus_field(&mut self, path: &str, value: bool) {
        self.focus.insert(path.to_string(), value);
    }
}

fn literal_value_to_json(value: &IrStaticValue) -> Value {
    match value {
        IrStaticValue::Text { value } => json!(value),
        IrStaticValue::Number { value } => json!(value),
        IrStaticValue::Bool { value } => json!(value),
        IrStaticValue::Tag { value } => json!(value),
        IrStaticValue::Path { value } => json!(value),
        IrStaticValue::Range { from, to } => json!({
            "from": from,
            "to": to,
        }),
        IrStaticValue::Record { fields } => json!(
            fields
                .iter()
                .map(|field| (field.key.clone(), literal_value_to_json(&field.value)))
                .collect::<BTreeMap<_, _>>()
        ),
        IrStaticValue::List { items } => {
            json!(items.iter().map(literal_value_to_json).collect::<Vec<_>>())
        }
    }
}

fn grid_dimensions_from_static_records(app_ir: &AppIr) -> (usize, usize) {
    let rows = top_static_range_end(app_ir, "rows").unwrap_or(100);
    let columns = top_static_range_end(app_ir, "columns").unwrap_or(26);
    (rows, columns)
}

fn top_static_range_end(app_ir: &AppIr, path: &str) -> Option<usize> {
    let record = app_ir
        .static_records
        .iter()
        .find(|record| record.path == path)?;
    match &record.fields.first()?.value {
        IrStaticValue::Range { to, .. } => usize::try_from(*to).ok().filter(|value| *value > 0),
        _ => None,
    }
}

fn std_function_names_from_static_records(app_ir: &AppIr) -> Vec<String> {
    let mut names = BTreeSet::new();
    for record in &app_ir.static_records {
        collect_std_function_names(&record.fields, &mut names);
    }
    names.into_iter().collect()
}

fn collect_std_function_names(fields: &[IrStaticField], names: &mut BTreeSet<String>) {
    for field in fields {
        match &field.value {
            IrStaticValue::Path { value } if value.starts_with("Math/") => {
                names.insert(field.key.clone());
            }
            IrStaticValue::Record { fields } => collect_std_function_names(fields, names),
            IrStaticValue::List { items } => {
                for item in items {
                    collect_std_function_names_from_value(item, names);
                }
            }
            _ => {}
        }
    }
}

fn collect_std_function_names_from_value(value: &IrStaticValue, names: &mut BTreeSet<String>) {
    match value {
        IrStaticValue::Record { fields } => collect_std_function_names(fields, names),
        IrStaticValue::List { items } => {
            for item in items {
                collect_std_function_names_from_value(item, names);
            }
        }
        IrStaticValue::Text { .. }
        | IrStaticValue::Number { .. }
        | IrStaticValue::Bool { .. }
        | IrStaticValue::Tag { .. }
        | IrStaticValue::Path { .. }
        | IrStaticValue::Range { .. } => {}
    }
}

impl CompiledApp {
    pub fn new(compiled: boon_compiler::CompiledModule) -> Self {
        let inventory = compiled.sources;
        let program = compiled.program;
        let app_ir = compiled.app_ir;
        let executable_ir = compiled.executable_ir;
        let wiring = RuntimeWiring::from_compiled(&app_ir, &executable_ir, &inventory);
        let initial_records = app_ir
            .collection_states
            .first()
            .map(|list| list.initial_entries.clone())
            .unwrap_or_default();
        let (grid_rows, grid_columns) = grid_dimensions_from_static_records(&app_ir);
        let grid = FormulaBook::new(
            grid_rows,
            grid_columns,
            std_function_names_from_static_records(&app_ir),
        );
        let initial_view_selector = collection_view_from_app_ir(&app_ir).map_or_else(
            || "all".to_string(),
            |view| {
                view.selectors
                    .first()
                    .map(|selector| selector.id.clone())
                    .unwrap_or_else(|| "all".to_string())
            },
        );
        let mut generic_state = BTreeMap::new();
        for slot in &executable_ir.state_slots {
            if let Ok(value) = selfless_eval_exec_number(&slot.initial) {
                generic_state.insert(slot.path.clone(), value);
            }
        }
        let mut app = Self {
            program,
            app_ir,
            executable_ir,
            inventory,
            wiring,
            turn: 0,
            frame_text: String::new(),
            clock: RuntimeClock::default(),
            dynamic_values: initial_records
                .into_iter()
                .enumerate()
                .map(|(idx, item)| {
                    RuntimeDynamicValue::from_literal_fields(idx as u64 + 1, &item.fields)
                })
                .collect(),
            next_dynamic_value_id: 1,
            entry_text: String::new(),
            source_state: BTreeMap::new(),
            generic_state,
            view_selector: initial_view_selector,
            grid,
            grid_selected: (1, 1),
            grid_edit_focus: None,
        };
        app.next_dynamic_value_id = app.dynamic_values.len() as u64 + 1;
        app.frame_text = app.render_text();
        app
    }

    pub fn validate_source_batch(&self, batch: &SourceBatch) -> Result<()> {
        self.validate_batch(batch)
    }

    fn emit_frame(&mut self, changed: &[&str], mut metrics: TurnMetrics) -> TurnResult {
        self.turn += 1;
        self.frame_text = self.render_text();
        let patches = self.frame_patches();
        metrics.patch_count = patches.len();
        TurnResult {
            turn_id: TurnId(self.turn),
            patches,
            state_delta: StateDelta {
                changed_paths: changed.iter().map(|s| (*s).to_string()).collect(),
            },
            metrics,
        }
    }

    fn emit_frame_owned(&mut self, changed: Vec<String>, metrics: TurnMetrics) -> TurnResult {
        let refs = changed.iter().map(String::as_str).collect::<Vec<_>>();
        self.emit_frame(&refs, metrics)
    }

    fn frame_patches(&self) -> Vec<HostPatch> {
        vec![
            HostPatch::ReplaceFrameText {
                text: self.frame_text.clone(),
            },
            HostPatch::ReplaceFrameScene {
                scene: self.render_scene(),
            },
        ]
    }

    fn record_root(&self) -> Option<&str> {
        self.wiring
            .list
            .as_ref()
            .map(|sequence| sequence.root.as_str())
    }

    fn record_state_prefix(&self) -> Option<String> {
        self.record_root().map(|root| format!("store.{root}"))
    }

    fn record_change_paths(&self) -> Vec<String> {
        self.record_state_prefix()
            .into_iter()
            .chain(
                self.wiring
                    .list
                    .as_ref()
                    .and_then(|sequence| sequence.entry_text.clone()),
            )
            .collect()
    }

    fn record_count_change_paths(&self) -> Vec<String> {
        let Some(root) = self.record_root() else {
            return Vec::new();
        };
        vec![
            format!("store.{root}_count"),
            format!("store.marked_{root}_count"),
            format!("store.unmarked_{root}_count"),
        ]
    }

    fn record_input_change_paths(&self) -> Vec<String> {
        self.wiring
            .list
            .as_ref()
            .and_then(|sequence| sequence.entry_text.clone())
            .into_iter()
            .collect()
    }

    fn grid_change_paths(&self) -> Vec<String> {
        self.wiring
            .grid
            .as_ref()
            .map(|grid| vec![grid.root.clone()])
            .unwrap_or_default()
    }

    fn grid_selection_change_paths(&self) -> Vec<String> {
        self.wiring
            .grid
            .as_ref()
            .map(|grid| vec![format!("{}.selected", grid.root)])
            .unwrap_or_default()
    }

    fn set_grid_selected(&mut self, row: usize, col: usize) {
        self.grid_selected = (
            row.clamp(1, self.grid.rows()),
            col.clamp(1, self.grid.columns()),
        );
    }

    fn move_grid_selected(&mut self, row_delta: isize, col_delta: isize) {
        let row = self
            .grid_selected
            .0
            .saturating_add_signed(row_delta)
            .clamp(1, self.grid.rows());
        let col = self
            .grid_selected
            .1
            .saturating_add_signed(col_delta)
            .clamp(1, self.grid.columns());
        self.grid_selected = (row, col);
    }

    fn static_text_event_matches(&self, path: &str) -> bool {
        self.wiring.list.as_ref().is_some_and(|sequence| {
            [
                &sequence.input_focus,
                &sequence.input_blur,
                &sequence.input_change,
            ]
            .into_iter()
            .flatten()
            .any(|candidate| candidate == path)
        })
    }

    fn dynamic_text_event_matches(&self, path: &str) -> bool {
        self.wiring.list.as_ref().is_some_and(|sequence| {
            [&sequence.dynamic_text_blur, &sequence.dynamic_text_change]
                .into_iter()
                .flatten()
                .any(|candidate| candidate == path)
        })
    }

    fn apply_generic_event(&mut self, event: &SourceEmission) -> Result<Option<Vec<String>>> {
        if let Some(changed) = self.apply_executable_event(event)? {
            return Ok(Some(changed));
        }

        let handlers = self
            .app_ir
            .event_handlers
            .iter()
            .filter(|handler| handler.source_path == event.path)
            .cloned()
            .collect::<Vec<_>>();
        if handlers.is_empty() {
            return Ok(None);
        }

        let mut changed = Vec::new();
        for handler in handlers {
            if !self.generic_predicate_matches(handler.when.as_ref(), event)? {
                continue;
            }
            for effect in handler.effects {
                match effect {
                    IrEffect::Assign { state_path, expr } => {
                        let value = self.eval_generic_number(&expr, event)?;
                        self.set_generic_number(&state_path, value);
                        changed.push(state_path);
                    }
                    IrEffect::CollectionAppendRecord {
                        collection_path,
                        fields,
                        skip_if_empty_field,
                    } => {
                        if self.apply_generic_collection_append_record(
                            &collection_path,
                            &fields,
                            skip_if_empty_field.as_deref(),
                        )? {
                            changed.extend(self.record_change_paths());
                            changed.extend(self.record_count_change_paths());
                        }
                    }
                    IrEffect::CollectionUpdateAllFields {
                        collection_path,
                        field,
                        value,
                    } => {
                        if self.apply_generic_collection_update_all_fields(
                            &collection_path,
                            &field,
                            &value,
                        )? {
                            changed.extend(self.record_count_change_paths());
                        }
                    }
                    IrEffect::CollectionUpdateOwnerField {
                        collection_path,
                        field,
                        value,
                    } => {
                        if self.apply_generic_collection_update_owner_field(
                            &collection_path,
                            &field,
                            &value,
                            event,
                        )? {
                            changed.extend(self.record_count_change_paths());
                        }
                    }
                    IrEffect::CollectionRemoveCurrent { collection_path } => {
                        if self.apply_generic_collection_remove_current(&collection_path, event)? {
                            changed.extend(self.record_change_paths());
                        }
                    }
                    IrEffect::CollectionRemoveWhere {
                        collection_path,
                        predicate,
                    } => {
                        if self.apply_generic_collection_remove_where(&collection_path, &predicate)
                        {
                            changed.extend(self.record_change_paths());
                        }
                    }
                    IrEffect::SetTagState { state_path, value } => {
                        if state_path == "view_selector" {
                            self.view_selector = value.clone();
                        }
                        changed.push(state_path);
                    }
                    IrEffect::ClearText { text_state_path } => {
                        self.clear_generic_text_state(&text_state_path);
                        changed.push(text_state_path);
                    }
                }
            }
        }
        Ok(Some(unique_paths(changed)))
    }

    fn apply_executable_event(&mut self, event: &SourceEmission) -> Result<Option<Vec<String>>> {
        let handlers = self
            .executable_ir
            .source_handlers
            .iter()
            .filter(|handler| handler.source_path == event.path)
            .cloned()
            .collect::<Vec<_>>();
        if handlers.is_empty() {
            return Ok(None);
        }

        let mut changed = Vec::new();
        let previous_state = self.generic_state.clone();
        let updates = handlers
            .into_iter()
            .flat_map(|handler| handler.effects)
            .map(|effect| match effect {
                ExecEffect::SetState { path, value } => {
                    let value = self.eval_exec_number_with_state(&value, event, &previous_state)?;
                    Ok((path, value))
                }
            })
            .collect::<Result<Vec<_>>>()?;
        for (path, value) in updates {
            self.generic_state.insert(path.clone(), value);
            changed.push(path);
        }
        Ok(Some(unique_paths(changed)))
    }

    fn generic_predicate_matches(
        &self,
        predicate: Option<&IrPredicate>,
        event: &SourceEmission,
    ) -> Result<bool> {
        let Some(predicate) = predicate else {
            return Ok(true);
        };
        match predicate {
            IrPredicate::SourceTagEquals { path, tag } => Ok(path == &event.path
                && matches!(&event.value, SourceValue::Tag(value) if value == tag)),
        }
    }

    fn eval_generic_number(&self, expr: &IrValueExpr, event: &SourceEmission) -> Result<i64> {
        match expr {
            IrValueExpr::Number { value } => Ok(*value),
            IrValueExpr::Hold { state_path } => {
                Ok(*self.generic_state.get(state_path).unwrap_or(&0))
            }
            IrValueExpr::Add { left, right } => {
                Ok(self.eval_generic_number(left, event)?
                    + self.eval_generic_number(right, event)?)
            }
            IrValueExpr::Source { path } => match &event.value {
                SourceValue::Number(value) if path == &event.path => Ok(*value),
                _ => bail!("generic numeric source `{path}` did not emit a number"),
            },
            IrValueExpr::Skip => bail!("generic SKIP is not assignable to numeric state"),
        }
    }

    fn eval_exec_number_with_state(
        &self,
        expr: &ExecExpr,
        event: &SourceEmission,
        state: &BTreeMap<String, i64>,
    ) -> Result<i64> {
        match expr {
            ExecExpr::Number { value } => Ok(*value),
            ExecExpr::State { path } => Ok(*state.get(path).unwrap_or(&0)),
            ExecExpr::Source { path } => match &event.value {
                SourceValue::Number(value) if path == &event.path => Ok(*value),
                _ => bail!("executable numeric source `{path}` did not emit a number"),
            },
            ExecExpr::Add { left, right } => Ok(self
                .eval_exec_number_with_state(left, event, state)?
                + self.eval_exec_number_with_state(right, event, state)?),
            ExecExpr::Subtract { left, right } => Ok(self
                .eval_exec_number_with_state(left, event, state)?
                - self.eval_exec_number_with_state(right, event, state)?),
            ExecExpr::Call { path, args } => self.eval_exec_number_call(path, args, event, state),
            ExecExpr::When { arms } => {
                let Some(arm) = arms.iter().find(|arm| match &event.value {
                    SourceValue::Tag(value) => arm.pattern == *value || arm.pattern == "__",
                    _ => arm.pattern == "__",
                }) else {
                    bail!("executable WHEN did not contain a matching arm")
                };
                self.eval_exec_number_with_state(&arm.value, event, state)
            }
            ExecExpr::Equal { .. }
            | ExecExpr::Text { .. }
            | ExecExpr::Bool { .. }
            | ExecExpr::Tag { .. }
            | ExecExpr::TextFromNumber { .. }
            | ExecExpr::Skip => bail!("executable expression is not a number: {expr:?}"),
        }
    }

    fn eval_exec_number_call(
        &self,
        path: &str,
        args: &[boon_compiler::ExecCallArg],
        event: &SourceEmission,
        state: &BTreeMap<String, i64>,
    ) -> Result<i64> {
        boon_stdlib::eval_number_call(path, |name| {
            let value = args
                .iter()
                .find(|arg| arg.name == name)
                .ok_or_else(|| format!("missing `{name}` argument for `{path}`"))?;
            self.eval_exec_number_with_state(&value.value, event, state)
                .map_err(|err| err.to_string())
        })
        .map_err(|err| anyhow::anyhow!("{err}"))
    }

    fn set_generic_number(&mut self, state_path: &str, value: i64) {
        self.generic_state.insert(state_path.to_string(), value);
    }

    fn apply_generic_collection_append_record(
        &mut self,
        collection_path: &str,
        fields: &[boon_compiler::IrCollectionFieldAssignment],
        skip_if_empty_field: Option<&str>,
    ) -> Result<bool> {
        if self.record_root() != Some(collection_path) {
            return Ok(false);
        }
        let mut record = BTreeMap::new();
        for field in fields {
            let value = self.eval_collection_value(&field.value, None)?;
            record.insert(field.field.clone(), value);
        }
        if skip_if_empty_field
            .and_then(|field| record.get(field))
            .and_then(Value::as_str)
            .is_some_and(str::is_empty)
        {
            return Ok(false);
        }
        self.dynamic_values
            .push(RuntimeDynamicValue::new(self.next_dynamic_value_id, record));
        self.next_dynamic_value_id += 1;
        Ok(true)
    }

    fn apply_generic_collection_update_all_fields(
        &mut self,
        collection_path: &str,
        field: &str,
        value: &IrCollectionValueExpr,
    ) -> Result<bool> {
        if self.record_root() != Some(collection_path) {
            return Ok(false);
        }
        let values = self
            .dynamic_values
            .iter()
            .map(|record| self.eval_collection_value(value, Some(record)))
            .collect::<Result<Vec<_>>>()?;
        for (record, value) in self.dynamic_values.iter_mut().zip(values) {
            record.set_field_json(field, value);
        }
        Ok(true)
    }

    fn apply_generic_collection_update_owner_field(
        &mut self,
        collection_path: &str,
        field: &str,
        value: &IrCollectionValueExpr,
        event: &SourceEmission,
    ) -> Result<bool> {
        if self.record_root() != Some(collection_path) {
            return Ok(false);
        }
        let owner_id = event
            .owner_id
            .as_deref()
            .expect("dynamic event owner_id was validated");
        let dynamic_value_id = owner_id
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("dynamic value owner_id `{owner_id}` is not numeric"))?;
        let Some(index) = self
            .dynamic_values
            .iter()
            .position(|record| record.id == dynamic_value_id)
        else {
            return Ok(false);
        };
        let updated = self.eval_collection_value(value, Some(&self.dynamic_values[index]))?;
        self.dynamic_values[index].set_field_json(field, updated);
        Ok(true)
    }

    fn eval_collection_value(
        &self,
        expr: &IrCollectionValueExpr,
        owner: Option<&RuntimeDynamicValue>,
    ) -> Result<Value> {
        match expr {
            IrCollectionValueExpr::Static { value } => Ok(literal_value_to_json(value)),
            IrCollectionValueExpr::SourceText { path, trim } => {
                let mut text = if self
                    .wiring
                    .list
                    .as_ref()
                    .and_then(|sequence| sequence.entry_text.as_ref())
                    .is_some_and(|entry| entry == path)
                {
                    self.entry_text.clone()
                } else {
                    match self.source_state.get(path) {
                        Some(SourceValue::Text(value)) => value.clone(),
                        _ => String::new(),
                    }
                };
                if *trim {
                    text = text.trim().to_string();
                }
                Ok(json!(text))
            }
            IrCollectionValueExpr::NotOwnerBoolField { field } => {
                let owner = owner.context("owner field expression requires an owner record")?;
                Ok(json!(!owner.bool_field(field)))
            }
            IrCollectionValueExpr::NotAllBoolField { field } => {
                let all_marked = self
                    .dynamic_values
                    .iter()
                    .all(|record| record.bool_field(field));
                Ok(json!(!all_marked))
            }
        }
    }

    fn apply_generic_collection_remove_current(
        &mut self,
        collection_path: &str,
        event: &SourceEmission,
    ) -> Result<bool> {
        if self.record_root() != Some(collection_path) {
            return Ok(false);
        }
        let owner_id = event
            .owner_id
            .as_deref()
            .expect("dynamic event owner_id was validated");
        let dynamic_value_id = owner_id
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("dynamic value owner_id `{owner_id}` is not numeric"))?;
        let before = self.dynamic_values.len();
        self.dynamic_values
            .retain(|record| record.id != dynamic_value_id);
        Ok(self.dynamic_values.len() != before)
    }

    fn apply_generic_collection_remove_where(
        &mut self,
        collection_path: &str,
        predicate: &IrCollectionPredicate,
    ) -> bool {
        if self.record_root() != Some(collection_path) {
            return false;
        }
        let before = self.dynamic_values.len();
        match predicate {
            IrCollectionPredicate::FieldBoolEquals { field, value } => {
                self.dynamic_values
                    .retain(|record| record.bool_field(field) != *value);
            }
        }
        self.dynamic_values.len() != before
    }

    fn clear_generic_text_state(&mut self, text_state_path: &str) {
        if self
            .wiring
            .list
            .as_ref()
            .and_then(|sequence| sequence.entry_text.as_ref())
            .is_some_and(|path| path == text_state_path)
        {
            self.entry_text.clear();
        }
        self.source_state.insert(
            text_state_path.to_string(),
            SourceValue::Text(String::new()),
        );
    }

    fn list_view(&self) -> Option<CollectionView> {
        collection_view_from_app_ir(&self.app_ir)
    }

    fn collection_text_field(&self) -> &str {
        self.wiring
            .list
            .as_ref()
            .map(|sequence| sequence.text_field.as_str())
            .unwrap_or("value")
    }

    fn collection_bool_field(&self) -> Option<&str> {
        self.wiring
            .list
            .as_ref()
            .and_then(CollectionSourceWiring::bool_field)
    }

    fn collection_edit_focus_field(&self) -> Option<&str> {
        self.wiring
            .list
            .as_ref()
            .and_then(CollectionSourceWiring::edit_focus_field)
    }

    fn collection_record_marked(&self, record: &RuntimeDynamicValue) -> bool {
        self.collection_bool_field()
            .is_some_and(|field| record.bool_field(field))
    }

    fn spatial_source_enabled(&self) -> bool {
        self.spatial_state_path("body_x").is_some()
    }

    fn spatial_i64(&self, key: &str, fallback: i64) -> i64 {
        self.spatial_state_path(key)
            .and_then(|path| self.generic_state.get(&path))
            .copied()
            .unwrap_or(fallback)
    }

    fn spatial_state_path(&self, key: &str) -> Option<String> {
        let suffix = format!(".{key}");
        self.generic_state
            .keys()
            .find(|path| path.ends_with(&suffix))
            .cloned()
    }

    fn spatial_source_root(&self) -> Option<String> {
        self.spatial_state_path("body_x")
            .and_then(|path| path.strip_suffix(".body_x").map(str::to_string))
    }

    fn spatial_contact_mode(&self) -> bool {
        self.spatial_state_path("contact_field_live_count")
            .is_some()
    }

    fn has_render_kind(&self, kind: boon_compiler::IrRenderKind) -> bool {
        self.app_ir
            .render_tree
            .as_ref()
            .is_some_and(|node| render_node_has_kind(node, &kind))
    }

    fn action_value(&self) -> Option<i64> {
        self.wiring
            .action_state
            .as_ref()
            .and_then(|binding| self.generic_state.get(&binding.state_path).copied())
    }

    fn clock_value(&self) -> Option<i64> {
        self.wiring
            .clock_state
            .as_ref()
            .and_then(|binding| self.generic_state.get(&binding.state_path).copied())
    }

    fn render_text(&self) -> String {
        if self.has_render_kind(boon_compiler::IrRenderKind::Grid) {
            self.render_slot_text()
        } else if self.spatial_source_enabled() {
            self.render_spatial_text()
        } else if self.can_render_generic_scene() {
            self.render_generic_text()
        } else {
            String::new()
        }
    }

    fn render_scene(&self) -> FrameScene {
        let mut scene = FrameScene {
            title: self.program.title.clone(),
            commands: Vec::new(),
            hit_targets: Vec::new(),
        };
        push_rect(&mut scene, 0, 0, 1000, 1000, [245, 245, 245, 255]);
        if self.has_render_kind(boon_compiler::IrRenderKind::Grid) {
            self.render_element_grid_scene(&mut scene);
        } else if self.spatial_source_enabled() {
            self.render_spatial_scene(&mut scene);
        } else if self.can_render_generic_scene() {
            self.render_generic_scene(&mut scene);
        }
        scene
    }

    fn can_render_generic_scene(&self) -> bool {
        self.app_ir.render_tree.is_some()
            && !self.has_render_kind(boon_compiler::IrRenderKind::Grid)
            && !self.spatial_source_enabled()
    }

    fn render_generic_text(&self) -> String {
        let mut lines = vec![
            self.program.title.clone(),
            "surface: generic_scene".to_string(),
        ];
        for record in &self.app_ir.static_records {
            collect_static_text_values(&record.fields, &mut lines);
        }
        if let Some(tree) = &self.app_ir.render_tree {
            self.collect_generic_text(tree, &mut lines);
        }
        lines.join("\n")
    }

    fn collect_generic_text(&self, node: &boon_compiler::IrRenderNode, lines: &mut Vec<String>) {
        self.collect_generic_text_with_record(node, lines, None);
    }

    fn collect_generic_text_with_record(
        &self,
        node: &boon_compiler::IrRenderNode,
        lines: &mut Vec<String>,
        record: Option<&RuntimeDynamicValue>,
    ) {
        if matches!(node.kind, boon_compiler::IrRenderKind::ListMap) {
            for record in self.visible_dynamic_values() {
                lines.push(format!(
                    "{} [{}] {}",
                    record.id,
                    if self.collection_record_marked(record) {
                        "x"
                    } else {
                        " "
                    },
                    record.text_field(self.collection_text_field())
                ));
                for child in &node.children {
                    self.collect_generic_text_with_record(child, lines, Some(record));
                }
            }
            return;
        }

        if matches!(node.kind, boon_compiler::IrRenderKind::TextInput)
            && let Some(source_path) = node.source_path.as_deref()
        {
            if let Some(record) = record {
                lines.push(record.text_field(self.collection_text_field()));
            } else {
                let text_state_path = format!("{source_path}.text");
                if let Some(SourceValue::Text(value)) = self.source_state.get(&text_state_path) {
                    lines.push(value.clone());
                }
            }
        }

        if let Some(text) = node.text.as_ref().and_then(|text| {
            record
                .and_then(|record| self.eval_render_text_for_record(text, record))
                .or_else(|| self.eval_render_text(text))
        }) {
            lines.push(text);
        }
        for child in &node.children {
            self.collect_generic_text_with_record(child, lines, record);
        }
    }

    fn render_generic_scene(&self, scene: &mut FrameScene) {
        push_rect(scene, 0, 0, 1000, 1000, [238, 244, 247, 255]);
        push_text(scene, 84, 108, 3, &self.program.title, [25, 40, 52, 255]);
        if let Some(tree) = &self.app_ir.render_tree {
            let mut y = 278;
            for child in &tree.children {
                y = self.render_generic_node(scene, child, 278, y, 444, None);
            }
        }
    }

    fn render_generic_node(
        &self,
        scene: &mut FrameScene,
        node: &boon_compiler::IrRenderNode,
        x: u32,
        y: u32,
        width: u32,
        record: Option<&RuntimeDynamicValue>,
    ) -> u32 {
        match node.kind {
            boon_compiler::IrRenderKind::Root | boon_compiler::IrRenderKind::Panel => {
                if let Some(record) = record {
                    return self.render_generic_record_row(scene, node, x, y, width, record);
                }
                let mut next_y = y;
                for child in &node.children {
                    next_y = self.render_generic_node(scene, child, x, next_y, width, record);
                }
                next_y
            }
            boon_compiler::IrRenderKind::Button => {
                push_rect(scene, x, y, width, 64, [46, 125, 166, 255]);
                push_rect_outline(scene, x, y, width, 64, [21, 91, 128, 255]);
                if let Some(source_path) = node
                    .source_path
                    .as_deref()
                    .and_then(|base| self.primary_event_source(base))
                {
                    self.push_generic_hit_target(
                        scene,
                        format!("generic_{}", node.id),
                        x,
                        y,
                        width,
                        64,
                        HitTargetAction::Press,
                        &source_path,
                        record,
                    );
                }
                let label = node
                    .text
                    .as_ref()
                    .and_then(|text| {
                        record
                            .and_then(|record| self.eval_render_text_for_record(text, record))
                            .or_else(|| self.eval_render_text(text))
                    })
                    .or_else(|| node.source_path.as_deref().map(generic_source_label))
                    .unwrap_or_default();
                push_text(scene, x + 18, y + 24, 1, &label, [255, 255, 255, 255]);
                y + 86
            }
            boon_compiler::IrRenderKind::Label
            | boon_compiler::IrRenderKind::Text
            | boon_compiler::IrRenderKind::Unknown => {
                let text = node
                    .text
                    .as_ref()
                    .and_then(|text| {
                        record
                            .and_then(|record| self.eval_render_text_for_record(text, record))
                            .or_else(|| self.eval_render_text(text))
                    })
                    .unwrap_or_default();
                if !text.is_empty() {
                    push_text(scene, x, y, 3, &text, [35, 55, 68, 255]);
                }
                y + 72
            }
            boon_compiler::IrRenderKind::TextInput => {
                push_rect(scene, x, y, width, 64, [255, 255, 255, 255]);
                push_rect_outline(scene, x, y, width, 64, [188, 202, 212, 255]);
                if let Some(source_path) = node.source_path.as_deref() {
                    let text_state_path = format!("{source_path}.text");
                    let text_value = record
                        .map(|record| record.text_field(self.collection_text_field()))
                        .or_else(|| {
                            self.wiring
                                .list
                                .as_ref()
                                .and_then(|sequence| sequence.entry_text.as_deref())
                                .filter(|entry| *entry == text_state_path)
                                .map(|_| self.entry_text.clone())
                        });
                    self.push_generic_text_hit_target(
                        scene,
                        format!("generic_{}", node.id),
                        x,
                        y,
                        width,
                        64,
                        source_path,
                        text_value.clone(),
                        record,
                    );
                    let display = text_value.unwrap_or_else(|| generic_source_label(source_path));
                    push_text(scene, x + 18, y + 24, 1, &display, [42, 58, 70, 255]);
                }
                y + 86
            }
            boon_compiler::IrRenderKind::Checkbox => {
                push_rect(scene, x, y, 64, 64, [255, 255, 255, 255]);
                push_rect_outline(scene, x, y, 64, 64, [188, 202, 212, 255]);
                if record.is_some_and(|record| self.collection_record_marked(record)) {
                    push_text(scene, x + 24, y + 24, 1, "x", [68, 146, 126, 255]);
                }
                if let Some(source_path) = node
                    .source_path
                    .as_deref()
                    .and_then(|base| self.primary_event_source(base))
                {
                    self.push_generic_hit_target(
                        scene,
                        format!("generic_{}", node.id),
                        x,
                        y,
                        64,
                        64,
                        HitTargetAction::Press,
                        &source_path,
                        record,
                    );
                }
                y + 86
            }
            boon_compiler::IrRenderKind::ListMap => {
                let mut next_y = y;
                for record in self.visible_dynamic_values() {
                    for child in &node.children {
                        next_y =
                            self.render_generic_node(scene, child, x, next_y, width, Some(record));
                    }
                }
                next_y
            }
            boon_compiler::IrRenderKind::Grid => y,
        }
    }

    fn render_generic_record_row(
        &self,
        scene: &mut FrameScene,
        node: &boon_compiler::IrRenderNode,
        x: u32,
        y: u32,
        width: u32,
        record: &RuntimeDynamicValue,
    ) -> u32 {
        let mut cursor = x;
        let mut remaining = width;
        for child in &node.children {
            if remaining == 0 {
                break;
            }
            let desired = match child.kind {
                boon_compiler::IrRenderKind::Checkbox => 64,
                boon_compiler::IrRenderKind::Button => 78,
                boon_compiler::IrRenderKind::TextInput
                | boon_compiler::IrRenderKind::Text
                | boon_compiler::IrRenderKind::Label
                | boon_compiler::IrRenderKind::Unknown => remaining,
                boon_compiler::IrRenderKind::Root
                | boon_compiler::IrRenderKind::Panel
                | boon_compiler::IrRenderKind::ListMap
                | boon_compiler::IrRenderKind::Grid => remaining,
            }
            .min(remaining);
            self.render_generic_inline_node(scene, child, cursor, y, desired, record);
            let step = desired.saturating_add(12).min(remaining);
            cursor = cursor.saturating_add(step);
            remaining = width.saturating_sub(cursor.saturating_sub(x));
        }
        y + 86
    }

    fn render_generic_inline_node(
        &self,
        scene: &mut FrameScene,
        node: &boon_compiler::IrRenderNode,
        x: u32,
        y: u32,
        width: u32,
        record: &RuntimeDynamicValue,
    ) {
        match node.kind {
            boon_compiler::IrRenderKind::Checkbox => {
                push_rect(scene, x, y, width.min(64), 64, [255, 255, 255, 255]);
                push_rect_outline(scene, x, y, width.min(64), 64, [188, 202, 212, 255]);
                if self.collection_record_marked(record) {
                    push_text(scene, x + 24, y + 24, 1, "x", [68, 146, 126, 255]);
                }
                if let Some(source_path) = node
                    .source_path
                    .as_deref()
                    .and_then(|base| self.primary_event_source(base))
                {
                    self.push_generic_hit_target(
                        scene,
                        format!("generic_{}", node.id),
                        x,
                        y,
                        width.min(64),
                        64,
                        HitTargetAction::Press,
                        &source_path,
                        Some(record),
                    );
                }
            }
            boon_compiler::IrRenderKind::Button => {
                push_rect(scene, x, y, width, 64, [46, 125, 166, 255]);
                push_rect_outline(scene, x, y, width, 64, [21, 91, 128, 255]);
                if let Some(source_path) = node
                    .source_path
                    .as_deref()
                    .and_then(|base| self.primary_event_source(base))
                {
                    self.push_generic_hit_target(
                        scene,
                        format!("generic_{}", node.id),
                        x,
                        y,
                        width,
                        64,
                        HitTargetAction::Press,
                        &source_path,
                        Some(record),
                    );
                }
                let label = node
                    .text
                    .as_ref()
                    .and_then(|text| self.eval_render_text_for_record(text, record))
                    .or_else(|| node.source_path.as_deref().map(generic_source_label))
                    .unwrap_or_default();
                push_text(scene, x + 12, y + 24, 1, &label, [255, 255, 255, 255]);
            }
            boon_compiler::IrRenderKind::TextInput => {
                push_rect(scene, x, y, width, 64, [255, 255, 255, 255]);
                push_rect_outline(scene, x, y, width, 64, [188, 202, 212, 255]);
                if let Some(source_path) = node.source_path.as_deref() {
                    let text_value = record.text_field(self.collection_text_field());
                    self.push_generic_text_hit_target(
                        scene,
                        format!("generic_{}", node.id),
                        x,
                        y,
                        width,
                        64,
                        source_path,
                        Some(text_value.clone()),
                        Some(record),
                    );
                    push_text(scene, x + 18, y + 24, 1, &text_value, [42, 58, 70, 255]);
                }
            }
            boon_compiler::IrRenderKind::Text
            | boon_compiler::IrRenderKind::Label
            | boon_compiler::IrRenderKind::Unknown => {
                if let Some(text) = node
                    .text
                    .as_ref()
                    .and_then(|text| self.eval_render_text_for_record(text, record))
                {
                    push_text(scene, x, y + 24, 1, &text, [35, 55, 68, 255]);
                }
            }
            boon_compiler::IrRenderKind::Root | boon_compiler::IrRenderKind::Panel => {
                let mut cursor = x;
                let mut remaining = width;
                for child in &node.children {
                    if remaining == 0 {
                        break;
                    }
                    let desired = match child.kind {
                        boon_compiler::IrRenderKind::Checkbox => 64,
                        boon_compiler::IrRenderKind::Button => 78,
                        _ => remaining,
                    }
                    .min(remaining);
                    self.render_generic_inline_node(scene, child, cursor, y, desired, record);
                    let step = desired.saturating_add(12).min(remaining);
                    cursor = cursor.saturating_add(step);
                    remaining = width.saturating_sub(cursor.saturating_sub(x));
                }
            }
            boon_compiler::IrRenderKind::ListMap | boon_compiler::IrRenderKind::Grid => {}
        }
    }

    fn eval_render_text(&self, text: &boon_compiler::IrRenderText) -> Option<String> {
        match text {
            boon_compiler::IrRenderText::Literal { value } => Some(value.clone()),
            boon_compiler::IrRenderText::Binding { path } => self.value_text(path),
        }
    }

    fn eval_render_text_for_record(
        &self,
        text: &boon_compiler::IrRenderText,
        record: &RuntimeDynamicValue,
    ) -> Option<String> {
        match text {
            boon_compiler::IrRenderText::Literal { value } => Some(value.clone()),
            boon_compiler::IrRenderText::Binding { path } => path
                .strip_prefix("item.")
                .map(|field| record.text_field(field))
                .or_else(|| self.value_text(path)),
        }
    }

    fn value_text(&self, path: &str) -> Option<String> {
        self.generic_state
            .get(path)
            .map(i64::to_string)
            .or_else(|| match self.source_state.get(path) {
                Some(SourceValue::Text(value)) => Some(value.clone()),
                Some(SourceValue::Number(value)) => Some(value.to_string()),
                Some(SourceValue::Tag(value)) => Some(value.clone()),
                Some(SourceValue::EmptyRecord) | None => None,
            })
    }

    fn primary_event_source(&self, base: &str) -> Option<String> {
        [
            format!("{base}.event.press"),
            format!("{base}.event.click"),
            format!("{base}.event.tick"),
            base.to_string(),
        ]
        .into_iter()
        .find(|candidate| self.inventory.get(candidate).is_some())
    }

    #[allow(clippy::too_many_arguments)]
    fn push_generic_hit_target(
        &self,
        scene: &mut FrameScene,
        id: impl Into<String>,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        action: HitTargetAction,
        source_path: &str,
        record: Option<&RuntimeDynamicValue>,
    ) {
        let target = HitTarget {
            id: id.into(),
            x,
            y,
            width,
            height,
            action,
            source_path: source_path.to_string(),
            owner_id: None,
            generation: 0,
            text_state_path: None,
            text_value: None,
            key_event_path: None,
            change_event_path: None,
            focus_event_path: None,
            blur_event_path: None,
        };
        scene
            .hit_targets
            .push(self.attach_generic_owner(target, source_path, record));
    }

    #[allow(clippy::too_many_arguments)]
    fn push_generic_text_hit_target(
        &self,
        scene: &mut FrameScene,
        id: impl Into<String>,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        source_base: &str,
        text_value: Option<String>,
        record: Option<&RuntimeDynamicValue>,
    ) {
        let text_state_path = format!("{source_base}.text");
        let target = HitTarget {
            id: id.into(),
            x,
            y,
            width,
            height,
            action: HitTargetAction::FocusText,
            source_path: text_state_path.clone(),
            owner_id: None,
            generation: 0,
            text_state_path: Some(text_state_path.clone()),
            text_value,
            key_event_path: existing_path(
                &self.inventory,
                &format!("{source_base}.event.key_down.key"),
            ),
            change_event_path: existing_path(
                &self.inventory,
                &format!("{source_base}.event.change"),
            ),
            focus_event_path: existing_path(&self.inventory, &format!("{source_base}.event.focus")),
            blur_event_path: existing_path(&self.inventory, &format!("{source_base}.event.blur")),
        };
        scene
            .hit_targets
            .push(self.attach_generic_owner(target, &text_state_path, record));
    }

    fn attach_generic_owner(
        &self,
        target: HitTarget,
        source_path: &str,
        record: Option<&RuntimeDynamicValue>,
    ) -> HitTarget {
        if source_path.contains("[*]")
            && let Some(record) = record
        {
            attach_owner(target, record.id.to_string(), record.generation)
        } else {
            target
        }
    }

    fn render_element_grid_scene(&self, scene: &mut FrameScene) {
        push_rect(scene, 0, 0, 1000, 1000, [248, 249, 250, 255]);
        let selected = format!(
            "{}{}",
            column_name(self.grid_selected.1),
            self.grid_selected.0
        );
        push_text(scene, 48, 34, 2, &self.program.title, [31, 46, 60, 255]);
        push_rect(scene, 48, 82, 904, 50, [255, 255, 255, 255]);
        push_rect_outline(scene, 48, 82, 904, 50, [188, 202, 212, 255]);
        push_text(scene, 64, 100, 1, &selected, [40, 64, 82, 255]);
        push_text(
            scene,
            142,
            100,
            1,
            self.slot_text(self.grid_selected.0, self.grid_selected.1),
            [35, 50, 64, 255],
        );
        let origin_x = 48;
        let origin_y = 160;
        let row_h = 38;
        let col_w = 92;
        let visible_cols = self.grid.columns().min(9) as u32;
        let visible_rows = self.grid.rows().min(15) as u32;
        push_rect(scene, origin_x, origin_y, 904, 40, [229, 235, 241, 255]);
        push_rect(scene, origin_x, origin_y, 52, 760, [229, 235, 241, 255]);
        for col in 1..=visible_cols {
            let x = origin_x + 52 + (col - 1) * col_w;
            push_rect_outline(scene, x, origin_y, col_w, 40, [196, 208, 216, 255]);
            push_text(
                scene,
                x + 36,
                origin_y + 15,
                1,
                &column_name(col as usize).to_string(),
                [62, 80, 96, 255],
            );
        }
        for row in 1..=visible_rows {
            let y = origin_y + 40 + (row - 1) * row_h;
            push_rect_outline(scene, origin_x, y, 52, row_h, [196, 208, 216, 255]);
            push_text(
                scene,
                origin_x + 18,
                y + 14,
                1,
                &row.to_string(),
                [62, 80, 96, 255],
            );
            for col in 1..=visible_cols {
                let x = origin_x + 52 + (col - 1) * col_w;
                let selected_slot = self.grid_selected == (row as usize, col as usize);
                push_rect(
                    scene,
                    x,
                    y,
                    col_w,
                    row_h,
                    if selected_slot {
                        [226, 242, 255, 255]
                    } else {
                        [255, 255, 255, 255]
                    },
                );
                push_rect_outline(
                    scene,
                    x,
                    y,
                    col_w,
                    row_h,
                    if selected_slot {
                        [57, 132, 198, 255]
                    } else {
                        [214, 222, 228, 255]
                    },
                );
                if let Some(grid) = &self.wiring.grid {
                    let owner_id = format!("{}{}", column_name(col as usize), row);
                    if let Some(path) = grid.display_double_click.as_deref() {
                        scene.hit_targets.push(attach_owner(
                            HitTarget {
                                id: format!("grid_slot_{owner_id}"),
                                x,
                                y,
                                width: col_w,
                                height: row_h,
                                action: HitTargetAction::FocusText,
                                source_path: path.to_string(),
                                owner_id: None,
                                generation: 0,
                                text_state_path: grid.editor_text.clone(),
                                text_value: Some(
                                    self.slot_text(row as usize, col as usize).to_string(),
                                ),
                                key_event_path: grid.editor_key.clone(),
                                change_event_path: None,
                                focus_event_path: None,
                                blur_event_path: None,
                            },
                            owner_id,
                            0,
                        ));
                    }
                }
                let value = self.slot_value(row as usize, col as usize);
                if !value.is_empty() {
                    push_text(scene, x + 8, y + 14, 1, value, [40, 55, 68, 255]);
                }
            }
        }
    }

    fn render_spatial_scene(&self, scene: &mut FrameScene) {
        if !self.spatial_source_enabled() {
            return;
        }
        let contact_mode = self.spatial_contact_mode();
        let arena_width = 1000;
        let arena_height = 700;
        let body_size = 22;
        let control_width = if contact_mode { 160 } else { 18 };
        let control_height = if contact_mode { 18 } else { 128 };
        push_rect(scene, 0, 0, 1000, 1000, [18, 24, 32, 255]);
        push_text(scene, 38, 28, 2, &self.program.title, [231, 241, 247, 255]);
        push_text(
            scene,
            38,
            66,
            1,
            &format!(
                "frame {} contact_value {} resets_remaining {}",
                self.spatial_i64("frame", 0),
                self.spatial_i64("contact_value", 0),
                self.spatial_i64("resets_remaining", 3)
            ),
            [153, 183, 198, 255],
        );
        let x0 = 56;
        let y0 = 118;
        let w = 888;
        let h = 622;
        push_rect(scene, x0, y0, w, h, [12, 20, 29, 255]);
        push_rect_outline(scene, x0, y0, w, h, [74, 103, 122, 255]);
        let sx = |value: i64| {
            x0 + ((value.clamp(0, arena_width) as u32) * w / arena_width.max(1) as u32)
        };
        let sy = |value: i64| {
            y0 + ((value.clamp(0, arena_height) as u32) * h / arena_height.max(1) as u32)
        };
        let sw = |value: i64| ((value.max(1) as u32) * w / arena_width.max(1) as u32).max(1);
        let sh = |value: i64| ((value.max(1) as u32) * h / arena_height.max(1) as u32).max(1);

        if contact_mode {
            let rows = usize::try_from(self.spatial_i64("contact_field_rows", 6).max(0))
                .unwrap_or_default();
            let columns =
                usize::try_from(self.spatial_i64("contact_field_cols", 12).max(1)).unwrap_or(12);
            let top = 56;
            let margin = 36;
            let gap = 8;
            let height = 28;
            let live_count = self
                .spatial_state_path("contact_field_live_count")
                .and_then(|path| self.generic_state.get(&path).copied())
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(rows.saturating_mul(columns));
            let contact_w = (arena_width - margin * 2 - (columns.saturating_sub(1) as i64 * gap))
                / columns.max(1) as i64;
            for row in 0..rows {
                for col in 0..columns {
                    let idx = row * columns + col;
                    if idx < live_count {
                        let bx = margin + col as i64 * (contact_w + gap);
                        let by = top + row as i64 * (height + gap);
                        let color = match row % 4 {
                            0 => [232, 92, 80, 255],
                            1 => [236, 168, 72, 255],
                            2 => [86, 176, 122, 255],
                            _ => [78, 146, 210, 255],
                        };
                        push_rect(scene, sx(bx), sy(by), sw(contact_w), sh(height), color);
                    }
                }
            }
        }

        let control_x = self.spatial_i64("control_x", 50);
        let control_y = self.spatial_i64("control_y", 50);
        let primary_x = if contact_mode {
            controller_left_from_position(control_x, arena_width, control_width)
        } else {
            38
        };
        let primary_y = if contact_mode {
            646
        } else {
            controller_top_from_position(control_y, arena_height, control_height)
        };
        push_rect(
            scene,
            sx(primary_x),
            sy(primary_y),
            sw(control_width),
            sh(control_height),
            [85, 212, 230, 255],
        );
        if !contact_mode {
            let tracked_x = 944;
            let tracked_h = 128;
            let tracked_w = 18;
            let tracked_y = controller_top_from_position(
                self.spatial_i64("tracked_control_y", 50),
                arena_height,
                tracked_h,
            );
            push_rect(
                scene,
                sx(tracked_x),
                sy(tracked_y),
                sw(tracked_w),
                sh(tracked_h),
                [240, 244, 247, 255],
            );
        }
        push_rect(
            scene,
            sx(self.spatial_i64("body_x", 500)),
            sy(self.spatial_i64("body_y", 350)),
            sw(body_size),
            sh(body_size),
            [250, 250, 250, 255],
        );
    }

    fn render_spatial_text(&self) -> String {
        let frame_source = first_static_source_path_matching(&self.inventory, ".event.frame")
            .unwrap_or_else(|| "frame source".to_string());
        format!(
            "{}\nsurface: spatial_scene\nspatial_mode: {}\nframe: {}\ncontrol_y: {}\ncontrol_x: {}\ntracked_control_y: {}\nbody_x: {}\nbody_y: {}\nbody_dx: {}\nbody_dy: {}\ncontact_field_rows: {}\ncontact_field_cols: {}\ncontact_field_live: {}\ncontact_value: {}\nresets_remaining: {}\ndeterministic input source: {}",
            self.program.title,
            if self.spatial_contact_mode() {
                "contact-grid"
            } else {
                "tracked-control"
            },
            self.spatial_i64("frame", 0),
            self.spatial_i64("control_y", 50),
            self.spatial_i64("control_x", 50),
            self.spatial_i64("tracked_control_y", 50),
            self.spatial_i64("body_x", 500),
            self.spatial_i64("body_y", 350),
            self.spatial_i64("body_dx", 10),
            self.spatial_i64("body_dy", 8),
            self.spatial_i64("contact_field_rows", 0),
            self.spatial_i64("contact_field_cols", 0),
            self.spatial_state_path("contact_field_live_count")
                .and_then(|path| self.generic_state.get(&path).copied())
                .map(|value| value.to_string())
                .unwrap_or_else(|| "0".to_string()),
            self.spatial_i64("contact_value", 0),
            self.spatial_i64("resets_remaining", 3),
            frame_source
        )
    }

    fn visible_dynamic_values(&self) -> impl Iterator<Item = &RuntimeDynamicValue> {
        let visibility = self
            .list_view()
            .and_then(|view| {
                view.selectors
                    .into_iter()
                    .find(|selector| selector.id == self.view_selector)
                    .map(|selector| selector.visibility)
            })
            .unwrap_or_default();
        self.dynamic_values
            .iter()
            .filter(move |record| match visibility {
                CollectionViewVisibility::All => true,
                CollectionViewVisibility::Unmarked => !self.collection_record_marked(record),
                CollectionViewVisibility::Marked => self.collection_record_marked(record),
            })
    }

    fn render_slot_text(&self) -> String {
        let mut lines = vec![
            self.program.title.clone(),
            "surface: grid_primitive".to_string(),
            format!(
                "selected: {}{}",
                column_name(self.grid_selected.1),
                self.grid_selected.0
            ),
            format!(
                "expression: {}",
                self.slot_text(self.grid_selected.0, self.grid_selected.1)
            ),
            format!(
                "value: {}",
                self.slot_value(self.grid_selected.0, self.grid_selected.1)
            ),
            "columns: A B C D E F ... Z".to_string(),
        ];
        for row in 1..=self.grid.rows().min(5) {
            lines.push(format!(
                "row {row}: A={} | B={} | C={}",
                self.slot_value(row, 1.min(self.grid.columns())),
                self.slot_value(row, 2.min(self.grid.columns())),
                self.slot_value(row, 3.min(self.grid.columns()))
            ));
        }
        lines.push(format!(
            "row {} and column {} reachable",
            self.grid.rows(),
            column_name(self.grid.columns())
        ));
        lines.join("\n")
    }

    fn slot_value(&self, row: usize, col: usize) -> &str {
        self.grid.value(row, col)
    }

    fn slot_text(&self, row: usize, col: usize) -> &str {
        self.grid.text(row, col)
    }

    fn parse_grid_owner(&self, owner_id: &str) -> Result<(usize, usize)> {
        self.grid
            .parse_owner(owner_id)
            .ok_or_else(|| anyhow::anyhow!("grid owner_id `{owner_id}` is outside compiled grid"))
    }

    fn validate_batch(&self, batch: &SourceBatch) -> Result<()> {
        for emission in batch.state_updates.iter().chain(batch.events.iter()) {
            self.validate_emission(emission)?;
        }
        Ok(())
    }

    fn validate_emission(&self, emission: &SourceEmission) -> Result<()> {
        let entry = self
            .inventory
            .get(&emission.path)
            .ok_or_else(|| anyhow::anyhow!("unknown SOURCE path `{}`", emission.path))?;
        validate_value_shape(&emission.value, &entry.shape, &emission.path)?;
        match &entry.owner {
            SourceOwner::Static => {
                if emission.owner_id.is_some() || emission.owner_generation.is_some() {
                    bail!(
                        "static SOURCE `{}` must not carry dynamic owner metadata",
                        emission.path
                    );
                }
            }
            SourceOwner::DynamicFamily { owner_path } => {
                let owner_id = emission.owner_id.as_deref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "dynamic SOURCE `{}` under `{owner_path}` is missing owner_id",
                        emission.path
                    )
                })?;
                let generation = emission.owner_generation.ok_or_else(|| {
                    anyhow::anyhow!(
                        "dynamic SOURCE `{}` for owner `{owner_id}` is missing owner_generation",
                        emission.path
                    )
                })?;
                let live_generation = self.live_generation(&emission.path, owner_id)?;
                if live_generation != generation {
                    bail!(
                        "stale dynamic SOURCE `{}` for owner `{owner_id}`: expected generation {live_generation}, got {generation}",
                        emission.path
                    );
                }
            }
        }
        Ok(())
    }

    fn live_generation(&self, path: &str, owner_id: &str) -> Result<u32> {
        if let Some(sequence) = &self.wiring.list
            && path.starts_with(&sequence.family)
        {
            let dynamic_value_id = owner_id.parse::<u64>().map_err(|_| {
                anyhow::anyhow!("dynamic value owner_id `{owner_id}` is not numeric")
            })?;
            return self
                .dynamic_values
                .iter()
                .find(|record| record.id == dynamic_value_id)
                .map(|record| record.generation)
                .ok_or_else(|| {
                    anyhow::anyhow!("dynamic dynamic value owner `{owner_id}` is not live")
                });
        }
        if let Some(grid) = &self.wiring.grid
            && path.starts_with(&grid.family)
        {
            self.parse_grid_owner(owner_id)?;
            return Ok(0);
        }
        bail!("dynamic SOURCE `{path}` has no owner generation grid")
    }
}

impl BoonApp for CompiledApp {
    fn mount(&mut self) -> TurnResult {
        let mut patches = vec![HostPatch::CreateNode {
            id: NodeId(0),
            kind: NodeKind::Root,
            parent: None,
            key: None,
        }];
        patches.extend(self.frame_patches());
        TurnResult {
            turn_id: TurnId(0),
            patches,
            state_delta: StateDelta::default(),
            metrics: TurnMetrics {
                patch_count: 3,
                ..TurnMetrics::default()
            },
        }
    }

    fn dispatch_batch(&mut self, batch: SourceBatch) -> Result<Vec<TurnResult>> {
        self.validate_batch(&batch)?;
        let mut changed_paths = Vec::new();
        for update in batch.state_updates {
            changed_paths.push(update.path.clone());
            self.source_state
                .insert(source_state_key(&update), update.value.clone());
            if self
                .wiring
                .list
                .as_ref()
                .and_then(|sequence| sequence.entry_text.as_ref())
                .is_some_and(|path| update.path == *path)
            {
                if let SourceValue::Text(value) = update.value {
                    self.entry_text = value;
                }
            } else if self
                .wiring
                .list
                .as_ref()
                .and_then(|sequence| sequence.dynamic_text_value.as_ref())
                .is_some_and(|path| update.path == *path)
                && let SourceValue::Text(value) = update.value
            {
                let owner_id = update
                    .owner_id
                    .as_deref()
                    .expect("dynamic edit text owner_id was validated");
                let dynamic_value_id = owner_id.parse::<u64>().map_err(|_| {
                    anyhow::anyhow!("dynamic value owner_id `{owner_id}` is not numeric")
                })?;
                let text_field = self.collection_text_field().to_string();
                let edit_focus_field = self.collection_edit_focus_field().map(str::to_string);
                let record = self
                    .dynamic_values
                    .iter_mut()
                    .find(|record| record.id == dynamic_value_id)
                    .ok_or_else(|| {
                        anyhow::anyhow!("dynamic value owner `{owner_id}` is not live")
                    })?;
                record.set_text_field(&text_field, value);
                if let Some(field) = edit_focus_field {
                    record.set_focus_field(&field, true);
                }
            } else if self
                .wiring
                .grid
                .as_ref()
                .and_then(|grid| grid.editor_text.as_ref())
                .is_some_and(|path| update.path == *path)
                && let SourceValue::Text(value) = update.value
            {
                let (row, col) = update
                    .owner_id
                    .as_deref()
                    .map(|owner_id| self.parse_grid_owner(owner_id))
                    .transpose()?
                    .unwrap_or(self.grid_selected);
                self.grid.set_text(row, col, value);
            }
        }

        let mut results = Vec::new();
        for event in batch.events {
            let metrics = TurnMetrics {
                events_processed: 1,
                ..TurnMetrics::default()
            };
            if let Some(changed) = self.apply_generic_event(&event)? {
                results.push(self.emit_frame_owned(changed, metrics));
            } else if self.static_text_event_matches(&event.path) {
                results.push(self.emit_frame_owned(self.record_input_change_paths(), metrics));
            } else if self
                .wiring
                .list
                .as_ref()
                .and_then(|sequence| sequence.dynamic_text_key.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                let owner_id = event
                    .owner_id
                    .as_deref()
                    .expect("dynamic edit_input key owner_id was validated");
                let dynamic_value_id = owner_id.parse::<u64>().map_err(|_| {
                    anyhow::anyhow!("dynamic value owner_id `{owner_id}` is not numeric")
                })?;
                let edit_focus_field = self.collection_edit_focus_field().map(str::to_string);
                if let Some(record) = self
                    .dynamic_values
                    .iter_mut()
                    .find(|record| record.id == dynamic_value_id)
                    && matches!(event.value, SourceValue::Tag(ref key) if key == "Enter")
                    && let Some(field) = edit_focus_field
                {
                    record.set_focus_field(&field, false);
                }
                results.push(self.emit_frame_owned(self.record_change_paths(), metrics));
            } else if self.dynamic_text_event_matches(&event.path) {
                let owner_id = event
                    .owner_id
                    .as_deref()
                    .expect("dynamic edit_input event owner_id was validated");
                let dynamic_value_id = owner_id.parse::<u64>().map_err(|_| {
                    anyhow::anyhow!("dynamic value owner_id `{owner_id}` is not numeric")
                })?;
                let edit_focus_field = self.collection_edit_focus_field().map(str::to_string);
                if let Some(record) = self
                    .dynamic_values
                    .iter_mut()
                    .find(|record| record.id == dynamic_value_id)
                    && event.path.ends_with(".event.blur")
                    && let Some(field) = edit_focus_field
                {
                    record.set_focus_field(&field, false);
                }
                results.push(self.emit_frame_owned(self.record_change_paths(), metrics));
            } else if self
                .wiring
                .grid
                .as_ref()
                .and_then(|grid| grid.display_double_click.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                let (row, col) = event
                    .owner_id
                    .as_deref()
                    .map(|owner_id| self.parse_grid_owner(owner_id))
                    .transpose()?
                    .unwrap_or(self.grid_selected);
                self.set_grid_selected(row, col);
                self.grid_edit_focus = Some((row, col));
                results.push(self.emit_frame_owned(self.grid_change_paths(), metrics));
            } else if self
                .wiring
                .grid
                .as_ref()
                .and_then(|grid| grid.editor_key.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                if matches!(event.value, SourceValue::Tag(ref key) if key == "Enter") {
                    self.grid_edit_focus = None;
                }
                results.push(self.emit_frame_owned(self.grid_change_paths(), metrics));
            } else if self
                .wiring
                .grid
                .as_ref()
                .and_then(|grid| grid.viewport_key.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                if let SourceValue::Tag(key) = &event.value {
                    match key.as_str() {
                        "ArrowUp" => self.move_grid_selected(-1, 0),
                        "ArrowDown" => self.move_grid_selected(1, 0),
                        "ArrowLeft" => self.move_grid_selected(0, -1),
                        "ArrowRight" => self.move_grid_selected(0, 1),
                        _ => {}
                    }
                }
                results.push(self.emit_frame_owned(self.grid_selection_change_paths(), metrics));
            }
        }
        if results.is_empty() && !changed_paths.is_empty() {
            let changed = changed_paths.iter().map(String::as_str).collect::<Vec<_>>();
            results.push(self.emit_frame(&changed, TurnMetrics::default()));
        }
        Ok(results)
    }

    fn advance_time(&mut self, delta: Duration) -> TurnResult {
        self.clock.advance(delta);
        let ticks = self.clock.millis / 1000;
        let Some(state_path) = self
            .wiring
            .clock_state
            .as_ref()
            .map(|binding| binding.state_path.clone())
        else {
            return self.emit_frame(&["clock"], TurnMetrics::default());
        };
        self.set_generic_number(&state_path, ticks as i64);
        self.emit_frame_owned(
            vec!["clock".to_string(), state_path],
            TurnMetrics::default(),
        )
    }

    fn snapshot(&self) -> AppSnapshot {
        let mark = self
            .dynamic_values
            .iter()
            .filter(|record| self.collection_record_marked(record))
            .count() as i64;
        let mut values = BTreeMap::new();
        for (path, value) in &self.generic_state {
            values.insert(path.clone(), json!(value));
        }
        if let Some(value) = self.action_value() {
            values.insert("scalar_value".to_string(), json!(value));
        }
        if let Some(root) = self.record_root() {
            values.insert(
                format!("store.{root}_count"),
                json!(self.dynamic_values.len() as i64),
            );
            values.insert(format!("store.marked_{root}_count"), json!(mark));
            values.insert(
                format!("store.unmarked_{root}_count"),
                json!(self.dynamic_values.len() as i64 - mark),
            );
        }
        if let Some(value) = self.clock_value() {
            values.insert("clock_value".to_string(), json!(value));
        }
        if let Some(sequence) = &self.wiring.list {
            if let Some(entry_text) = &sequence.entry_text {
                values.insert(entry_text.clone(), json!(self.entry_text));
            }
            values.insert(
                format!("store.{}_titles", sequence.root),
                json!(
                    self.dynamic_values
                        .iter()
                        .map(|record| record.text_field(self.collection_text_field()))
                        .collect::<Vec<_>>()
                ),
            );
            values.insert(
                format!("store.{}_ids", sequence.root),
                json!(
                    self.dynamic_values
                        .iter()
                        .map(|record| record.id)
                        .collect::<Vec<_>>()
                ),
            );
            values.insert(
                format!("store.visible_{}_ids", sequence.root),
                json!(
                    self.visible_dynamic_values()
                        .map(|record| record.id)
                        .collect::<Vec<_>>()
                ),
            );
            values.insert(view_selector_state_key(), json!(self.view_selector));
            for record in &self.dynamic_values {
                values.insert(
                    format!("store.{}[{}].content_text", sequence.root, record.id),
                    json!(record.text_field(self.collection_text_field())),
                );
                values.insert(
                    format!("store.{}[{}].mark", sequence.root, record.id),
                    json!(self.collection_record_marked(record)),
                );
                values.insert(
                    format!("store.{}[{}].edit_focus", sequence.root, record.id),
                    json!(
                        self.collection_edit_focus_field()
                            .is_some_and(|field| record.focus_field(field))
                    ),
                );
            }
        }
        if self.spatial_source_enabled() {
            let root = self
                .spatial_source_root()
                .unwrap_or_else(|| "spatial".to_string());
            values.insert(format!("{root}.frame"), json!(self.spatial_i64("frame", 0)));
            values.insert(
                format!("{root}.control_y"),
                json!(self.spatial_i64("control_y", 50)),
            );
            values.insert(
                format!("{root}.control_x"),
                json!(self.spatial_i64("control_x", 50)),
            );
            values.insert(
                format!("{root}.tracked_control_y"),
                json!(self.spatial_i64("tracked_control_y", 50)),
            );
            values.insert(
                format!("{root}.body_x"),
                json!(self.spatial_i64("body_x", 500)),
            );
            values.insert(
                format!("{root}.body_y"),
                json!(self.spatial_i64("body_y", 350)),
            );
            values.insert(
                format!("{root}.body_dx"),
                json!(self.spatial_i64("body_dx", 10)),
            );
            values.insert(
                format!("{root}.body_dy"),
                json!(self.spatial_i64("body_dy", 8)),
            );
            values.insert(
                format!("{root}.contact_field_rows"),
                json!(self.spatial_i64("contact_field_rows", 0)),
            );
            values.insert(
                format!("{root}.contact_field_cols"),
                json!(self.spatial_i64("contact_field_cols", 0)),
            );
            values.insert(
                format!("{root}.contact_field_live_count"),
                json!(self.spatial_i64("contact_field_live_count", 0)),
            );
            values.insert(
                format!("{root}.contact_value"),
                json!(self.spatial_i64("contact_value", 0)),
            );
            values.insert(
                format!("{root}.resets_remaining"),
                json!(self.spatial_i64("resets_remaining", 3)),
            );
        }
        let grid_root = self
            .wiring
            .grid
            .as_ref()
            .map(|grid| grid.root.as_str())
            .unwrap_or("grid");
        if self.wiring.grid.is_some() {
            for row in 1..=self.grid.rows() {
                for col in 1..=self.grid.columns() {
                    let coordinate = format!("{}{}", column_name(col), row);
                    values.insert(
                        format!("{grid_root}.{coordinate}"),
                        json!(self.slot_value(row, col)),
                    );
                    values.insert(
                        format!("{grid_root}.{coordinate}.expression"),
                        json!(self.slot_text(row, col)),
                    );
                }
            }
        }
        values.insert(
            format!("{grid_root}.selected_expression"),
            json!(self.slot_text(self.grid_selected.0, self.grid_selected.1)),
        );
        values.insert(
            format!("{grid_root}.selected_value"),
            json!(self.slot_value(self.grid_selected.0, self.grid_selected.1)),
        );
        values.insert(
            format!("{grid_root}.selected"),
            json!(format!(
                "{}{}",
                column_name(self.grid_selected.1),
                self.grid_selected.0
            )),
        );
        values.insert(
            format!("{grid_root}.edit_focus"),
            json!(
                self.grid_edit_focus
                    .map(|(row, col)| format!("{}{}", column_name(col), row))
            ),
        );
        AppSnapshot {
            values,
            frame_text: self.frame_text.clone(),
        }
    }

    fn source_inventory(&self) -> SourceInventory {
        self.inventory.clone()
    }
}

fn column_name(col: usize) -> char {
    (b'A' + (col as u8).saturating_sub(1)) as char
}

fn push_rect(scene: &mut FrameScene, x: u32, y: u32, width: u32, height: u32, color: [u8; 4]) {
    scene.commands.push(DrawCommand::Rect {
        x,
        y,
        width,
        height,
        color,
    });
}

fn push_rect_outline(
    scene: &mut FrameScene,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: [u8; 4],
) {
    scene.commands.push(DrawCommand::RectOutline {
        x,
        y,
        width,
        height,
        color,
    });
}

fn push_text(scene: &mut FrameScene, x: u32, y: u32, scale: u32, text: &str, color: [u8; 4]) {
    scene.commands.push(DrawCommand::Text {
        x,
        y,
        scale,
        text: text.to_string(),
        color,
    });
}

fn attach_owner(mut target: HitTarget, owner_id: impl Into<String>, generation: u32) -> HitTarget {
    target.owner_id = Some(owner_id.into());
    target.generation = generation;
    target
}

fn view_selector_state_key() -> String {
    "store.view_selector".to_string()
}

fn render_node_has_kind(
    node: &boon_compiler::IrRenderNode,
    kind: &boon_compiler::IrRenderKind,
) -> bool {
    &node.kind == kind
        || node
            .children
            .iter()
            .any(|child| render_node_has_kind(child, kind))
}

fn controller_top_from_position(position: i64, arena_h: i64, controller_h: i64) -> i64 {
    ((arena_h - controller_h).max(0) * position.clamp(0, 100) / 100)
        .clamp(0, arena_h - controller_h)
}

fn controller_left_from_position(position: i64, arena_w: i64, controller_w: i64) -> i64 {
    ((arena_w - controller_w).max(0) * position.clamp(0, 100) / 100)
        .clamp(0, arena_w - controller_w)
}

fn source_state_key(emission: &SourceEmission) -> String {
    match &emission.owner_id {
        Some(owner_id) => format!("{}#{owner_id}", emission.path),
        None => emission.path.clone(),
    }
}

fn validate_value_shape(value: &SourceValue, shape: &Shape, path: &str) -> Result<()> {
    let valid = match (value, shape) {
        (SourceValue::EmptyRecord, Shape::EmptyRecord) => true,
        (SourceValue::Text(_), Shape::Text) => true,
        (SourceValue::Number(_), Shape::Number) => true,
        (SourceValue::Tag(tag), Shape::TagSet(tags)) => tags.iter().any(|allowed| allowed == tag),
        (_, Shape::Union(shapes)) => shapes
            .iter()
            .any(|candidate| validate_value_shape(value, candidate, path).is_ok()),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        bail!(
            "SOURCE `{path}` expected {} but received {:?}",
            shape.label(),
            value
        )
    }
}
