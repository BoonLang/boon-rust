use crate::{
    AppSnapshot, BoonApp, RuntimeClock, SourceBatch, SourceEmission, SourceInventory, SourceValue,
    StateDelta, TurnId, TurnMetrics, TurnResult,
};
use anyhow::{Result, bail};
use boon_compiler::{
    AppIr, ControlAxis, ExecEffect, ExecExpr, ExecutableIr, IrAppMetadata, IrEffect, IrPredicate,
    IrStaticField, IrStaticRecord, IrStaticValue, IrValueExpr,
};
use boon_render_ir::{
    DrawCommand, FrameScene, HitTarget, HitTargetAction, HostPatch, NodeId, NodeKind,
};
use boon_shape::Shape;
use boon_source::{SourceEntry, SourceOwner};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const LIST_TEXT_FIELD: &str = "title";
const LIST_MARK_FIELD: &str = "completed";
const LIST_EDIT_FOCUS: &str = "sources.edit_input";

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
    grid: MatrixRuntimeState,
    frame_index: u64,
    motion: DynamicsRuntimeState,
}

#[derive(Clone, Debug, Default)]
struct RuntimeWiring {
    action_state: Option<StateEventBinding>,
    clock_state: Option<StateEventBinding>,
    list: Option<RepeaterBinding>,
    grid: Option<DenseBinding>,
    kinematic_frame_event: Option<String>,
    kinematic_control_event: Option<String>,
}

#[derive(Clone, Debug)]
struct StateEventBinding {
    state_path: String,
}

#[derive(Clone, Debug)]
struct RepeaterBinding {
    family: String,
    root: String,
    entry_text: Option<String>,
    input_key: Option<String>,
    input_focus: Option<String>,
    input_blur: Option<String>,
    input_change: Option<String>,
    static_mass_mark_event: Option<String>,
    static_remove_marked_event: Option<String>,
    view_selector_events: BTreeMap<String, String>,
    dynamic_mark_event: Option<String>,
    dynamic_remove_event: Option<String>,
    dynamic_text_value: Option<String>,
    dynamic_text_key: Option<String>,
    dynamic_text_blur: Option<String>,
    dynamic_text_change: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct RuntimeListView {
    title_line: String,
    entry_hint: String,
    count_suffix: String,
    remove_marked_label: Option<String>,
    auxiliary_lines: Vec<String>,
    selectors: Vec<RuntimeListSelector>,
}

#[derive(Clone, Debug)]
struct RuntimeListSelector {
    id: String,
    label: String,
    visibility: RuntimeListVisibility,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RuntimeListVisibility {
    #[default]
    All,
    Unmarked,
    Marked,
}

#[derive(Clone, Debug)]
struct DenseBinding {
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
        let list = RepeaterBinding::from_app_ir(inventory, app_ir);
        let grid = (!app_ir.matrix_models.is_empty())
            .then(|| DenseBinding::from_inventory(inventory))
            .flatten();
        let kinematic_frame_event = app_ir
            .dynamics_models
            .first()
            .map(|kinematics| kinematics.frame_event_path.clone());
        let kinematic_control_event = app_ir
            .dynamics_models
            .first()
            .map(|kinematics| kinematics.control_event_path.clone());
        Self {
            action_state,
            clock_state,
            list,
            grid,
            kinematic_frame_event,
            kinematic_control_event,
        }
    }
}

impl RepeaterBinding {
    fn from_app_ir(inventory: &SourceInventory, app_ir: &AppIr) -> Option<Self> {
        if !app_ir.event_handlers.iter().any(|handler| {
            handler.effects.iter().any(|effect| {
                matches!(
                    effect,
                    IrEffect::ListAppendText { .. }
                        | IrEffect::ListToggleAllMarks { .. }
                        | IrEffect::ListToggleOwnerMark { .. }
                        | IrEffect::ListRemoveOwner { .. }
                        | IrEffect::ListRemoveMarked { .. }
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
            .or_else(|| first_list_effect_path(app_ir))
            .or_else(|| app_ir.list_states.first().map(|list| list.path.clone()))?;
        let entry_text = app_ir.event_handlers.iter().find_map(|handler| {
            handler.effects.iter().find_map(|effect| match effect {
                IrEffect::ListAppendText {
                    text_state_path, ..
                } => Some(text_state_path.clone()),
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
        let mut view_selector_events = BTreeMap::new();
        let mut static_mass_mark_event = None;
        let mut static_remove_marked_event = None;
        let mut dynamic_mark_event = None;
        let mut dynamic_remove_event = None;
        for handler in &app_ir.event_handlers {
            for effect in &handler.effects {
                match effect {
                    IrEffect::SetTagState { value, .. } => {
                        view_selector_events.insert(value.clone(), handler.source_path.clone());
                    }
                    IrEffect::ListToggleAllMarks { .. } => {
                        static_mass_mark_event = Some(handler.source_path.clone());
                    }
                    IrEffect::ListRemoveMarked { .. } => {
                        static_remove_marked_event = Some(handler.source_path.clone());
                    }
                    IrEffect::ListToggleOwnerMark { .. } => {
                        dynamic_mark_event = Some(handler.source_path.clone());
                    }
                    IrEffect::ListRemoveOwner { .. } => {
                        dynamic_remove_event = Some(handler.source_path.clone());
                    }
                    _ => {}
                }
            }
        }
        let family = dynamic_family.unwrap_or_else(|| format!("{root}[*]"));
        Some(Self {
            family: family.clone(),
            root,
            entry_text: entry_text
                .or_else(|| input_base.as_ref().map(|base| format!("{base}.text"))),
            input_key: input_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.key_down.key"))),
            input_focus: input_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.focus"))),
            input_blur: input_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.blur"))),
            input_change: input_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.change"))),
            static_mass_mark_event,
            static_remove_marked_event,
            view_selector_events,
            dynamic_mark_event: dynamic_mark_event.or_else(|| {
                dynamic_path_for_producer(
                    inventory,
                    &family,
                    "Element/checkbox(element.event.click)",
                )
            }),
            dynamic_remove_event: dynamic_remove_event.or_else(|| {
                dynamic_path_for_producer(inventory, &family, "Element/button(element.event.press)")
            }),
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
}

impl DenseBinding {
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

fn first_list_effect_path(app_ir: &AppIr) -> Option<String> {
    app_ir.event_handlers.iter().find_map(|handler| {
        handler.effects.iter().find_map(|effect| match effect {
            IrEffect::ListAppendText { list_path, .. }
            | IrEffect::ListToggleAllMarks { list_path }
            | IrEffect::ListToggleOwnerMark { list_path }
            | IrEffect::ListRemoveOwner { list_path }
            | IrEffect::ListRemoveMarked { list_path } => Some(list_path.clone()),
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

fn runtime_list_view_from_app_ir(app_ir: &AppIr) -> Option<RuntimeListView> {
    let view = app_ir
        .static_records
        .iter()
        .find(|record| record.path == "view")?;
    Some(RuntimeListView {
        title_line: static_text_field(view, "title_line").unwrap_or_default(),
        entry_hint: static_text_field(view, "entry_hint").unwrap_or_default(),
        count_suffix: static_text_field(view, "count_suffix").unwrap_or_else(|| "items".into()),
        remove_marked_label: static_record_field(view, "actions")
            .and_then(|actions| static_record_value_field(actions, "remove_marked"))
            .and_then(|action| static_value_text_field(action, "label")),
        auxiliary_lines: static_record_field(view, "auxiliary")
            .map(|auxiliary| {
                auxiliary
                    .iter()
                    .filter_map(|field| static_value_text(&field.value))
                    .filter(|line| !line.is_empty())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default(),
        selectors: static_record_field(view, "selectors")
            .map(|selectors| {
                selectors
                    .iter()
                    .map(|field| RuntimeListSelector {
                        id: field.key.clone(),
                        label: static_value_text_field(&field.value, "label")
                            .unwrap_or_else(|| field.key.clone()),
                        visibility: static_value_field(&field.value, "visibility")
                            .and_then(static_value_tag)
                            .and_then(runtime_list_visibility_from_tag)
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn static_text_field(record: &IrStaticRecord, field: &str) -> Option<String> {
    static_record_value_field(&record.fields, field)
        .and_then(static_value_text)
        .cloned()
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

fn static_value_text_field(value: &IrStaticValue, field: &str) -> Option<String> {
    static_value_field(value, field)
        .and_then(static_value_text)
        .cloned()
}

fn static_value_record(value: &IrStaticValue) -> Option<&[IrStaticField]> {
    match value {
        IrStaticValue::Record { fields } => Some(fields),
        _ => None,
    }
}

fn static_value_text(value: &IrStaticValue) -> Option<&String> {
    match value {
        IrStaticValue::Text { value } => Some(value),
        _ => None,
    }
}

fn static_value_tag(value: &IrStaticValue) -> Option<&str> {
    match value {
        IrStaticValue::Tag { value } => Some(value.as_str()),
        _ => None,
    }
}

fn runtime_list_visibility_from_tag(value: &str) -> Option<RuntimeListVisibility> {
    match value {
        "All" => Some(RuntimeListVisibility::All),
        "Unmarked" => Some(RuntimeListVisibility::Unmarked),
        "Marked" => Some(RuntimeListVisibility::Marked),
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
    fn new(id: u64, text: String, completed: bool) -> Self {
        let mut fields = BTreeMap::new();
        fields.insert(LIST_TEXT_FIELD.to_string(), json!(text));
        fields.insert(LIST_MARK_FIELD.to_string(), json!(completed));
        Self {
            id,
            generation: 0,
            fields,
            focus: BTreeMap::new(),
        }
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

    fn set_bool_field(&mut self, path: &str, value: bool) {
        self.fields.insert(path.to_string(), json!(value));
    }

    fn focus_field(&self, path: &str) -> bool {
        self.focus.get(path).copied().unwrap_or(false)
    }

    fn set_focus_field(&mut self, path: &str, value: bool) {
        self.focus.insert(path.to_string(), value);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MatrixRuntimeState {
    rows: usize,
    columns: usize,
    selected: (usize, usize),
    edit_focus: Option<(usize, usize)>,
    text: Vec<String>,
    value: Vec<String>,
    deps: Vec<Vec<usize>>,
    rev_deps: Vec<Vec<usize>>,
}

impl MatrixRuntimeState {
    fn new(rows: usize, columns: usize) -> Self {
        let len = rows * columns;
        Self {
            rows,
            columns,
            selected: (1, 1),
            edit_focus: None,
            text: vec![String::new(); len],
            value: vec![String::new(); len],
            deps: vec![Vec::new(); len],
            rev_deps: vec![Vec::new(); len],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DynamicsRuntimeState {
    body_x: i64,
    body_y: i64,
    body_dx: i64,
    body_dy: i64,
    control_x: i64,
    control_y: i64,
    tracked_control_y: i64,
    contact_field_rows: usize,
    contact_field_cols: usize,
    contact_field: Vec<bool>,
    contact_value: i64,
    resets_remaining: i64,
}

impl DynamicsRuntimeState {
    fn from_model(model: Option<&boon_compiler::IrDynamicsModel>) -> Self {
        let Some(model) = model else {
            return Self::default();
        };
        let contact_field_rows = model
            .contact_field
            .as_ref()
            .map_or(0, |contact_field| contact_field.rows);
        let contact_field_cols = model
            .contact_field
            .as_ref()
            .map_or(0, |contact_field| contact_field.columns);
        Self {
            body_x: model.body.x,
            body_y: model.body.y,
            body_dx: model.body.dx,
            body_dy: model.body.dy,
            control_x: if matches!(model.primary_control.axis, ControlAxis::Horizontal) {
                model.primary_control.position
            } else {
                50
            },
            control_y: if matches!(model.primary_control.axis, ControlAxis::Vertical) {
                model.primary_control.position
            } else {
                50
            },
            tracked_control_y: model
                .tracked_control
                .as_ref()
                .map_or(50, |tracked_control| tracked_control.position),
            contact_field_rows,
            contact_field_cols,
            contact_field: vec![true; contact_field_rows * contact_field_cols],
            contact_value: 0,
            resets_remaining: 3,
        }
    }

    fn live_contact_field_indices(&self) -> String {
        self.contact_field
            .iter()
            .enumerate()
            .filter_map(|(idx, live)| live.then_some(idx.to_string()))
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl Default for DynamicsRuntimeState {
    fn default() -> Self {
        Self {
            body_x: 0,
            body_y: 0,
            body_dx: 0,
            body_dy: 0,
            control_x: 50,
            control_y: 50,
            tracked_control_y: 50,
            contact_field_rows: 0,
            contact_field_cols: 0,
            contact_field: Vec::new(),
            contact_value: 0,
            resets_remaining: 3,
        }
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
            .list_states
            .first()
            .map(|list| list.initial_entries.clone())
            .unwrap_or_default();
        let grid = app_ir
            .matrix_models
            .first()
            .map(|grid| MatrixRuntimeState::new(grid.rows, grid.columns))
            .unwrap_or_else(|| MatrixRuntimeState::new(100, 26));
        let motion = DynamicsRuntimeState::from_model(app_ir.dynamics_models.first());
        let initial_view_selector = runtime_list_view_from_app_ir(&app_ir).map_or_else(
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
                    RuntimeDynamicValue::new(
                        idx as u64 + 1,
                        item.text.unwrap_or_default(),
                        item.mark,
                    )
                })
                .collect(),
            next_dynamic_value_id: 1,
            entry_text: String::new(),
            source_state: BTreeMap::new(),
            generic_state,
            view_selector: initial_view_selector,
            grid,
            frame_index: 0,
            motion,
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

    fn dense_change_paths(&self) -> Vec<String> {
        self.wiring
            .grid
            .as_ref()
            .map(|grid| vec![grid.root.clone()])
            .unwrap_or_default()
    }

    fn dense_selection_change_paths(&self) -> Vec<String> {
        self.wiring
            .grid
            .as_ref()
            .map(|grid| vec![format!("{}.selected", grid.root)])
            .unwrap_or_default()
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
                    IrEffect::ListAppendText {
                        list_path,
                        text_state_path,
                        trim,
                        skip_empty,
                    } => {
                        if self.apply_generic_list_append_text(
                            &list_path,
                            &text_state_path,
                            trim,
                            skip_empty,
                        )? {
                            changed.extend(self.record_change_paths());
                            changed.extend(self.record_count_change_paths());
                        }
                    }
                    IrEffect::ListToggleAllMarks { list_path } => {
                        if self.apply_generic_list_mark_all(&list_path) {
                            changed.extend(self.record_count_change_paths());
                        }
                    }
                    IrEffect::ListToggleOwnerMark { list_path } => {
                        if self.apply_generic_list_toggle_owner_mark(&list_path, event)? {
                            changed.extend(self.record_count_change_paths());
                        }
                    }
                    IrEffect::ListRemoveOwner { list_path } => {
                        if self.apply_generic_list_remove_owner(&list_path, event)? {
                            changed.extend(self.record_change_paths());
                        }
                    }
                    IrEffect::ListRemoveMarked { list_path } => {
                        if self.apply_generic_list_remove_marked(&list_path) {
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
        for handler in handlers {
            for effect in handler.effects {
                match effect {
                    ExecEffect::SetState { path, value } => {
                        let value = self.eval_exec_number(&value, event)?;
                        self.generic_state.insert(path.clone(), value);
                        changed.push(path);
                    }
                }
            }
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

    fn eval_exec_number(&self, expr: &ExecExpr, event: &SourceEmission) -> Result<i64> {
        match expr {
            ExecExpr::Number { value } => Ok(*value),
            ExecExpr::State { path } => Ok(*self.generic_state.get(path).unwrap_or(&0)),
            ExecExpr::Source { path } => match &event.value {
                SourceValue::Number(value) if path == &event.path => Ok(*value),
                _ => bail!("executable numeric source `{path}` did not emit a number"),
            },
            ExecExpr::Add { left, right } => {
                Ok(self.eval_exec_number(left, event)? + self.eval_exec_number(right, event)?)
            }
            ExecExpr::Subtract { left, right } => {
                Ok(self.eval_exec_number(left, event)? - self.eval_exec_number(right, event)?)
            }
            ExecExpr::Equal { .. }
            | ExecExpr::Text { .. }
            | ExecExpr::Bool { .. }
            | ExecExpr::Tag { .. }
            | ExecExpr::TextFromNumber { .. }
            | ExecExpr::Skip => bail!("executable expression is not a number: {expr:?}"),
        }
    }

    fn set_generic_number(&mut self, state_path: &str, value: i64) {
        self.generic_state.insert(state_path.to_string(), value);
    }

    fn apply_generic_list_append_text(
        &mut self,
        list_path: &str,
        text_state_path: &str,
        trim: bool,
        skip_empty: bool,
    ) -> Result<bool> {
        if self.record_root() != Some(list_path) {
            return Ok(false);
        }
        let mut text = if self
            .wiring
            .list
            .as_ref()
            .and_then(|sequence| sequence.entry_text.as_ref())
            .is_some_and(|path| path == text_state_path)
        {
            self.entry_text.clone()
        } else {
            match self.source_state.get(text_state_path) {
                Some(SourceValue::Text(value)) => value.clone(),
                _ => String::new(),
            }
        };
        if trim {
            text = text.trim().to_string();
        }
        if skip_empty && text.is_empty() {
            return Ok(false);
        }
        self.dynamic_values.push(RuntimeDynamicValue::new(
            self.next_dynamic_value_id,
            text,
            false,
        ));
        self.next_dynamic_value_id += 1;
        Ok(true)
    }

    fn apply_generic_list_mark_all(&mut self, list_path: &str) -> bool {
        if self.record_root() != Some(list_path) {
            return false;
        }
        let all_marked = self
            .dynamic_values
            .iter()
            .all(|record| record.bool_field(LIST_MARK_FIELD));
        for record in &mut self.dynamic_values {
            record.set_bool_field(LIST_MARK_FIELD, !all_marked);
        }
        true
    }

    fn apply_generic_list_toggle_owner_mark(
        &mut self,
        list_path: &str,
        event: &SourceEmission,
    ) -> Result<bool> {
        if self.record_root() != Some(list_path) {
            return Ok(false);
        }
        let owner_id = event
            .owner_id
            .as_deref()
            .expect("dynamic event owner_id was validated");
        let dynamic_value_id = owner_id
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("dynamic value owner_id `{owner_id}` is not numeric"))?;
        let Some(record) = self
            .dynamic_values
            .iter_mut()
            .find(|record| record.id == dynamic_value_id)
        else {
            return Ok(false);
        };
        record.set_bool_field(LIST_MARK_FIELD, !record.bool_field(LIST_MARK_FIELD));
        Ok(true)
    }

    fn apply_generic_list_remove_owner(
        &mut self,
        list_path: &str,
        event: &SourceEmission,
    ) -> Result<bool> {
        if self.record_root() != Some(list_path) {
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

    fn apply_generic_list_remove_marked(&mut self, list_path: &str) -> bool {
        if self.record_root() != Some(list_path) {
            return false;
        }
        let before = self.dynamic_values.len();
        self.dynamic_values
            .retain(|record| !record.bool_field(LIST_MARK_FIELD));
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

    fn list_view(&self) -> Option<RuntimeListView> {
        runtime_list_view_from_app_ir(&self.app_ir)
    }

    fn dynamics_model(&self) -> Option<&boon_compiler::IrDynamicsModel> {
        self.app_ir.dynamics_models.first()
    }

    fn has_render_kind(&self, kind: boon_compiler::IrRenderKind) -> bool {
        self.app_ir
            .render_tree
            .as_ref()
            .is_some_and(|node| render_node_has_kind(node, &kind))
    }

    fn has_repeater_surface(&self) -> bool {
        self.wiring.list.is_some()
            && (self.has_render_kind(boon_compiler::IrRenderKind::ListMap)
                || self.list_view().is_some())
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
        if self.has_repeater_surface() {
            self.render_repeater_text()
        } else if self.has_render_kind(boon_compiler::IrRenderKind::Grid) {
            self.render_matrix_text()
        } else if !self.app_ir.dynamics_models.is_empty() {
            self.render_dynamics_text()
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
        if self.has_repeater_surface() {
            self.render_repeater_scene(&mut scene);
        } else if self.has_render_kind(boon_compiler::IrRenderKind::Grid) {
            self.render_matrix_scene(&mut scene);
        } else if !self.app_ir.dynamics_models.is_empty() {
            self.render_dynamics_scene(&mut scene);
        } else if self.can_render_generic_scene() {
            self.render_generic_scene(&mut scene);
        }
        scene
    }

    fn can_render_generic_scene(&self) -> bool {
        self.app_ir.render_tree.is_some()
            && !self.has_repeater_surface()
            && !self.has_render_kind(boon_compiler::IrRenderKind::Grid)
            && self.app_ir.dynamics_models.is_empty()
    }

    fn render_generic_text(&self) -> String {
        let mut lines = vec![
            self.program.title.clone(),
            "surface: generic_scene".to_string(),
        ];
        if let Some(tree) = &self.app_ir.render_tree {
            self.collect_generic_text(tree, &mut lines);
        }
        lines.join("\n")
    }

    fn collect_generic_text(&self, node: &boon_compiler::IrRenderNode, lines: &mut Vec<String>) {
        if let Some(text) = node
            .text
            .as_ref()
            .and_then(|text| self.eval_render_text(text))
        {
            lines.push(text);
        }
        for child in &node.children {
            self.collect_generic_text(child, lines);
        }
    }

    fn render_generic_scene(&self, scene: &mut FrameScene) {
        push_rect(scene, 0, 0, 1000, 1000, [238, 244, 247, 255]);
        push_text(scene, 84, 108, 3, &self.program.title, [25, 40, 52, 255]);
        if let Some(tree) = &self.app_ir.render_tree {
            let mut y = 278;
            for child in &tree.children {
                y = self.render_generic_node(scene, child, 278, y, 444);
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
    ) -> u32 {
        match node.kind {
            boon_compiler::IrRenderKind::Root | boon_compiler::IrRenderKind::Panel => {
                let mut next_y = y;
                for child in &node.children {
                    next_y = self.render_generic_node(scene, child, x, next_y, width);
                }
                next_y
            }
            boon_compiler::IrRenderKind::Button => {
                push_rect(scene, x, y, width, 92, [46, 125, 166, 255]);
                push_rect_outline(scene, x, y, width, 92, [21, 91, 128, 255]);
                if let Some(source_path) = node
                    .source_path
                    .as_deref()
                    .and_then(|base| self.primary_event_source(base))
                {
                    push_hit_target(
                        scene,
                        format!("generic_{}", node.id),
                        x,
                        y,
                        width,
                        92,
                        HitTargetAction::Press,
                        &source_path,
                    );
                }
                let label = node
                    .text
                    .as_ref()
                    .and_then(|text| self.eval_render_text(text))
                    .unwrap_or_default();
                push_text(scene, x + 86, y + 36, 2, &label, [255, 255, 255, 255]);
                y + 124
            }
            boon_compiler::IrRenderKind::Label
            | boon_compiler::IrRenderKind::Text
            | boon_compiler::IrRenderKind::Unknown => {
                let text = node
                    .text
                    .as_ref()
                    .and_then(|text| self.eval_render_text(text))
                    .unwrap_or_default();
                if !text.is_empty() {
                    push_text(scene, x, y, 3, &text, [35, 55, 68, 255]);
                }
                y + 72
            }
            boon_compiler::IrRenderKind::TextInput | boon_compiler::IrRenderKind::Checkbox => {
                push_rect(scene, x, y, width, 64, [255, 255, 255, 255]);
                push_rect_outline(scene, x, y, width, 64, [188, 202, 212, 255]);
                y + 86
            }
            boon_compiler::IrRenderKind::Grid | boon_compiler::IrRenderKind::ListMap => y,
        }
    }

    fn eval_render_text(&self, text: &boon_compiler::IrRenderText) -> Option<String> {
        match text {
            boon_compiler::IrRenderText::Literal { value } => Some(value.clone()),
            boon_compiler::IrRenderText::Binding { path } => self.value_text(path),
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

    fn render_repeater_scene(&self, scene: &mut FrameScene) {
        let view = self.list_view();
        push_rect(scene, 0, 0, 1000, 1000, [245, 245, 245, 255]);
        push_text(
            scene,
            340,
            66,
            4,
            view.as_ref()
                .map(|view| view.title_line.as_str())
                .filter(|title_line| !title_line.is_empty())
                .unwrap_or(&self.program.title),
            [186, 137, 137, 255],
        );
        push_rect(scene, 206, 160, 588, 72, [255, 255, 255, 255]);
        push_rect_outline(scene, 206, 160, 588, 72, [225, 225, 225, 255]);
        push_rect_outline(scene, 226, 184, 28, 28, [198, 198, 198, 255]);
        if let Some(sequence) = &self.wiring.list {
            if let Some(text_path) = sequence.entry_text.as_deref() {
                push_text_hit_target(
                    scene,
                    "sequence_entry_text",
                    260,
                    160,
                    534,
                    72,
                    text_path,
                    text_path,
                    sequence.input_key.clone(),
                    sequence.input_change.clone(),
                    sequence.input_focus.clone(),
                    sequence.input_blur.clone(),
                );
            }
            if let Some(path) = sequence.static_mass_mark_event.as_deref() {
                push_hit_target(
                    scene,
                    "sequence_mass_mark",
                    206,
                    160,
                    54,
                    72,
                    HitTargetAction::Press,
                    path,
                );
            }
        }
        push_text(scene, 234, 191, 1, "v", [116, 116, 116, 255]);
        push_text(
            scene,
            274,
            186,
            2,
            if self.entry_text.is_empty() {
                view.as_ref()
                    .map(|view| view.entry_hint.as_str())
                    .unwrap_or_default()
            } else {
                &self.entry_text
            },
            if self.entry_text.is_empty() {
                [180, 180, 180, 255]
            } else {
                [54, 54, 54, 255]
            },
        );
        let visible_dynamic_values: Vec<_> = self.visible_dynamic_values().collect();
        let mut y = 234;
        for record in &visible_dynamic_values {
            if y >= 1000 {
                break;
            }
            push_rect(scene, 206, y, 588, 62, [255, 255, 255, 255]);
            push_rect_outline(scene, 206, y, 588, 62, [232, 232, 232, 255]);
            push_rect_outline(scene, 226, y + 18, 24, 24, [126, 178, 164, 255]);
            if let Some(sequence) = &self.wiring.list {
                if let Some(path) = sequence.dynamic_mark_event.as_deref() {
                    scene.hit_targets.push(attach_owner(
                        HitTarget {
                            id: format!("sequence_mark_{}", record.id),
                            x: 206,
                            y,
                            width: 58,
                            height: 62,
                            action: HitTargetAction::Press,
                            source_path: path.to_string(),
                            owner_id: None,
                            generation: 0,
                            text_state_path: None,
                            text_value: None,
                            key_event_path: None,
                            change_event_path: None,
                            focus_event_path: None,
                            blur_event_path: None,
                        },
                        record.id.to_string(),
                        record.generation,
                    ));
                }
                if let Some(path) = sequence.dynamic_remove_event.as_deref() {
                    scene.hit_targets.push(attach_owner(
                        HitTarget {
                            id: format!("sequence_remove_{}", record.id),
                            x: 736,
                            y,
                            width: 58,
                            height: 62,
                            action: HitTargetAction::Press,
                            source_path: path.to_string(),
                            owner_id: None,
                            generation: 0,
                            text_state_path: None,
                            text_value: None,
                            key_event_path: None,
                            change_event_path: None,
                            focus_event_path: None,
                            blur_event_path: None,
                        },
                        record.id.to_string(),
                        record.generation,
                    ));
                }
                if let Some(text_path) = sequence.dynamic_text_value.as_deref() {
                    scene.hit_targets.push(attach_owner(
                        HitTarget {
                            id: format!("sequence_text_{}", record.id),
                            x: 264,
                            y,
                            width: 472,
                            height: 62,
                            action: HitTargetAction::FocusText,
                            source_path: text_path.to_string(),
                            owner_id: None,
                            generation: 0,
                            text_state_path: Some(text_path.to_string()),
                            text_value: Some(record.text_field(LIST_TEXT_FIELD)),
                            key_event_path: sequence.dynamic_text_key.clone(),
                            change_event_path: sequence.dynamic_text_change.clone(),
                            focus_event_path: None,
                            blur_event_path: sequence.dynamic_text_blur.clone(),
                        },
                        record.id.to_string(),
                        record.generation,
                    ));
                }
            }
            if record.bool_field(LIST_MARK_FIELD) {
                push_text(scene, 231, y + 19, 1, "x", [68, 146, 126, 255]);
            }
            push_text(
                scene,
                270,
                y + 22,
                2,
                &record.text_field(LIST_TEXT_FIELD),
                if record.bool_field(LIST_MARK_FIELD) {
                    [160, 160, 160, 255]
                } else {
                    [60, 60, 60, 255]
                },
            );
            push_text(scene, 744, y + 22, 1, "x", [172, 84, 84, 255]);
            y += 62;
        }
        let mark = self
            .dynamic_values
            .iter()
            .filter(|record| record.bool_field(LIST_MARK_FIELD))
            .count();
        let unmarked = self.dynamic_values.len().saturating_sub(mark);
        y = 234 + visible_dynamic_values.len() as u32 * 62;
        if y >= 1000 {
            return;
        }
        push_rect(scene, 206, y, 588, 54, [255, 255, 255, 255]);
        push_rect_outline(scene, 206, y, 588, 54, [232, 232, 232, 255]);
        push_text(
            scene,
            230,
            y + 20,
            1,
            &format!(
                "{unmarked} {}",
                view.as_ref()
                    .map(|view| view.count_suffix.as_str())
                    .unwrap_or("unmarked")
            ),
            [116, 116, 116, 255],
        );
        let view_selectors = self.list_view_selector_layout();
        for (view_selector, x, outline_w, label) in view_selectors {
            if self.view_selector == view_selector {
                push_rect_outline(scene, x - 10, y + 12, outline_w, 28, [218, 185, 185, 255]);
            }
            if let Some(path) = self
                .wiring
                .list
                .as_ref()
                .and_then(|sequence| sequence.view_selector_events.get(&view_selector))
            {
                push_hit_target(
                    scene,
                    format!("sequence_selector_{view_selector}"),
                    x - 10,
                    y + 12,
                    outline_w,
                    28,
                    HitTargetAction::Press,
                    path,
                );
            }
            push_text(scene, x, y + 21, 1, &label, [116, 116, 116, 255]);
        }
        if mark > 0
            && let Some(label) = view
                .as_ref()
                .and_then(|view| view.remove_marked_label.as_deref())
        {
            if let Some(path) = self
                .wiring
                .list
                .as_ref()
                .and_then(|sequence| sequence.static_remove_marked_event.as_deref())
            {
                push_hit_target(
                    scene,
                    "sequence_remove_marked",
                    670,
                    y + 12,
                    124,
                    28,
                    HitTargetAction::Press,
                    path,
                );
            }
            push_text(scene, 680, y + 21, 1, label, [116, 116, 116, 255]);
        }
        if let Some(view) = view {
            for (index, line) in view.auxiliary_lines.iter().enumerate() {
                push_text(
                    scene,
                    338,
                    y + 82 + index as u32 * 34,
                    1,
                    line,
                    [150, 150, 150, 255],
                );
            }
        }
    }

    fn list_view_selector_layout(&self) -> Vec<(String, u32, u32, String)> {
        let view_selectors = self
            .list_view()
            .map(|view| view.selectors)
            .unwrap_or_default();
        let mut x = 366;
        view_selectors
            .into_iter()
            .map(|view_selector| {
                let label = if view_selector.label.is_empty() {
                    view_selector.id.clone()
                } else {
                    view_selector.label.clone()
                };
                let width = (label.chars().count() as u32 * 10 + 28).max(48);
                let selector = (view_selector.id, x, width, label);
                x += width + 22;
                selector
            })
            .collect()
    }

    fn render_matrix_scene(&self, scene: &mut FrameScene) {
        push_rect(scene, 0, 0, 1000, 1000, [248, 249, 250, 255]);
        let selected = format!(
            "{}{}",
            column_name(self.grid.selected.1),
            self.grid.selected.0
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
            self.dense_text(self.grid.selected.0, self.grid.selected.1),
            [35, 50, 64, 255],
        );
        let origin_x = 48;
        let origin_y = 160;
        let row_h = 38;
        let col_w = 92;
        let visible_cols = self.grid.columns.min(9) as u32;
        let visible_rows = self.grid.rows.min(15) as u32;
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
                let selected_slot = self.grid.selected == (row as usize, col as usize);
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
                                id: format!("dense_slot_{owner_id}"),
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
                                    self.dense_text(row as usize, col as usize).to_string(),
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
                let value = self.dense_value(row as usize, col as usize);
                if !value.is_empty() {
                    push_text(scene, x + 8, y + 14, 1, value, [40, 55, 68, 255]);
                }
            }
        }
    }

    fn render_dynamics_scene(&self, scene: &mut FrameScene) {
        let Some(kinematics) = self.dynamics_model() else {
            return;
        };
        push_rect(scene, 0, 0, 1000, 1000, [18, 24, 32, 255]);
        push_text(scene, 38, 28, 2, &self.program.title, [231, 241, 247, 255]);
        push_text(
            scene,
            38,
            66,
            1,
            &format!(
                "frame {} contact_value {} resets_remaining {}",
                self.frame_index, self.motion.contact_value, self.motion.resets_remaining
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
            x0 + ((value.clamp(0, kinematics.arena_width) as u32) * w
                / kinematics.arena_width.max(1) as u32)
        };
        let sy = |value: i64| {
            y0 + ((value.clamp(0, kinematics.arena_height) as u32) * h
                / kinematics.arena_height.max(1) as u32)
        };
        let sw =
            |value: i64| ((value.max(1) as u32) * w / kinematics.arena_width.max(1) as u32).max(1);
        let sh =
            |value: i64| ((value.max(1) as u32) * h / kinematics.arena_height.max(1) as u32).max(1);

        if let Some(contact_field) = &kinematics.contact_field {
            let contact_w = (kinematics.arena_width
                - contact_field.margin * 2
                - (contact_field.columns.saturating_sub(1) as i64 * contact_field.gap))
                / contact_field.columns.max(1) as i64;
            for row in 0..contact_field.rows {
                for col in 0..contact_field.columns {
                    let idx = row * contact_field.columns + col;
                    if self.motion.contact_field.get(idx).copied().unwrap_or(false) {
                        let bx =
                            contact_field.margin + col as i64 * (contact_w + contact_field.gap);
                        let by = contact_field.top
                            + row as i64 * (contact_field.height + contact_field.gap);
                        let color = match row % 4 {
                            0 => [232, 92, 80, 255],
                            1 => [236, 168, 72, 255],
                            2 => [86, 176, 122, 255],
                            _ => [78, 146, 210, 255],
                        };
                        push_rect(
                            scene,
                            sx(bx),
                            sy(by),
                            sw(contact_w),
                            sh(contact_field.height),
                            color,
                        );
                    }
                }
            }
        }

        let primary_x = if kinematics.primary_control.axis == ControlAxis::Horizontal {
            controller_left_from_position(
                self.motion.control_x,
                kinematics.arena_width,
                kinematics.primary_control.width,
            )
        } else {
            kinematics.primary_control.x
        };
        let primary_y = if kinematics.primary_control.axis == ControlAxis::Vertical {
            controller_top_from_position(
                self.motion.control_y,
                kinematics.arena_height,
                kinematics.primary_control.height,
            )
        } else {
            kinematics.primary_control.y
        };
        push_rect(
            scene,
            sx(primary_x),
            sy(primary_y),
            sw(kinematics.primary_control.width),
            sh(kinematics.primary_control.height),
            [85, 212, 230, 255],
        );
        if let Some(tracked_control) = &kinematics.tracked_control {
            let tracked_y = controller_top_from_position(
                self.motion.tracked_control_y,
                kinematics.arena_height,
                tracked_control.height,
            );
            push_rect(
                scene,
                sx(tracked_control.x),
                sy(tracked_y),
                sw(tracked_control.width),
                sh(tracked_control.height),
                [240, 244, 247, 255],
            );
        }
        push_rect(
            scene,
            sx(self.motion.body_x),
            sy(self.motion.body_y),
            sw(kinematics.body.size),
            sh(kinematics.body.size),
            [250, 250, 250, 255],
        );
    }

    fn render_repeater_text(&self) -> String {
        let view = self.list_view();
        let mark = self
            .dynamic_values
            .iter()
            .filter(|record| record.bool_field(LIST_MARK_FIELD))
            .count();
        let unmarked = self.dynamic_values.len().saturating_sub(mark);
        let mut lines = vec![
            self.program.title.clone(),
            "surface: sequence".to_string(),
            view.as_ref()
                .map(|view| view.entry_hint.clone())
                .unwrap_or_default(),
            format!("input: {}", self.entry_text),
        ];
        for record in self.visible_dynamic_values() {
            lines.push(format!(
                "{} [{}] {}",
                record.id,
                if record.bool_field(LIST_MARK_FIELD) {
                    "x"
                } else {
                    " "
                },
                record.text_field(LIST_TEXT_FIELD)
            ));
        }
        lines.push(format!(
            "{unmarked} {}",
            view.as_ref()
                .map(|view| view.count_suffix.as_str())
                .unwrap_or("unmarked")
        ));
        lines.push(format!("view_selector: {}", self.view_selector));
        if let Some(view) = view {
            lines.extend(view.auxiliary_lines.iter().cloned());
        }
        lines.join("\n")
    }

    fn render_dynamics_text(&self) -> String {
        let frame_source = self
            .wiring
            .kinematic_frame_event
            .as_deref()
            .unwrap_or("frame source");
        format!(
            "{}\nsurface: kinematics\nkinematic_mode: {}\nframe: {}\ncontrol_y: {}\ncontrol_x: {}\ntracked_control_y: {}\nbody_x: {}\nbody_y: {}\nbody_dx: {}\nbody_dy: {}\ncontact_field_rows: {}\ncontact_field_cols: {}\ncontact_field_live: {}\ncontact_value: {}\nresets_remaining: {}\ndeterministic input source: {}",
            self.program.title,
            if self
                .dynamics_model()
                .and_then(|kinematics| kinematics.contact_field.as_ref())
                .is_some()
            {
                "contact-field"
            } else {
                "dual-walls"
            },
            self.frame_index,
            self.motion.control_y,
            self.motion.control_x,
            self.motion.tracked_control_y,
            self.motion.body_x,
            self.motion.body_y,
            self.motion.body_dx,
            self.motion.body_dy,
            self.motion.contact_field_rows,
            self.motion.contact_field_cols,
            self.motion.live_contact_field_indices(),
            self.motion.contact_value,
            self.motion.resets_remaining,
            frame_source
        )
    }

    fn advance_kinematic_step(&mut self) {
        self.frame_index += 1;
        if self
            .dynamics_model()
            .and_then(|kinematics| kinematics.contact_field.as_ref())
            .is_some()
        {
            self.advance_bounded_contact_field_step();
        } else {
            self.advance_bounded_peer_step();
        }
    }

    fn advance_bounded_peer_step(&mut self) {
        let Some(kinematics) = self.dynamics_model().cloned() else {
            return;
        };
        let Some(tracked_control) = kinematics.tracked_control.as_ref() else {
            return;
        };
        let arena_w = kinematics.arena_width;
        let arena_h = kinematics.arena_height;
        let body_size = kinematics.body.size;
        let controller_w = kinematics.primary_control.width;
        let controller_h = kinematics.primary_control.height;
        let left_x = kinematics.primary_control.x;
        let right_x = tracked_control.x;

        self.motion.body_x += self.motion.body_dx;
        self.motion.body_y += self.motion.body_dy;
        if self.motion.body_y <= 0 {
            self.motion.body_y = 0;
            self.motion.body_dy = self.motion.body_dy.abs();
        } else if self.motion.body_y + body_size >= arena_h {
            self.motion.body_y = arena_h - body_size;
            self.motion.body_dy = -self.motion.body_dy.abs();
        }

        self.motion.tracked_control_y = position_from_controller_top(
            self.motion.body_y + body_size / 2 - controller_h / 2,
            arena_h,
            controller_h,
        );
        let left_y = controller_top_from_position(self.motion.control_y, arena_h, controller_h);
        let right_y =
            controller_top_from_position(self.motion.tracked_control_y, arena_h, controller_h);

        if self.motion.body_dx < 0
            && self.motion.body_x <= left_x + controller_w
            && self.motion.body_x + body_size >= left_x
            && ranges_overlap(
                self.motion.body_y,
                self.motion.body_y + body_size,
                left_y,
                left_y + controller_h,
            )
        {
            self.motion.body_x = left_x + controller_w;
            self.motion.body_dx = self.motion.body_dx.abs();
            self.motion.body_dy = (self.motion.body_dy
                + ((self.motion.body_y + body_size / 2) - (left_y + controller_h / 2)) / 18)
                .clamp(-18, 18);
            self.motion.contact_value += 1;
        }
        if self.motion.body_dx > 0
            && self.motion.body_x + body_size >= right_x
            && self.motion.body_x <= right_x + controller_w
            && ranges_overlap(
                self.motion.body_y,
                self.motion.body_y + body_size,
                right_y,
                right_y + controller_h,
            )
        {
            self.motion.body_x = right_x - body_size;
            self.motion.body_dx = -self.motion.body_dx.abs();
            self.motion.body_dy = (self.motion.body_dy
                + ((self.motion.body_y + body_size / 2) - (right_y + controller_h / 2)) / 18)
                .clamp(-18, 18);
            self.motion.contact_value += 1;
        }
        if self.motion.body_x < -body_size || self.motion.body_x > arena_w + body_size {
            self.motion.body_x = arena_w / 2;
            self.motion.body_y = arena_h / 2;
            self.motion.body_dx = if self.motion.body_dx < 0 { 12 } else { -12 };
            self.motion.body_dy = 8;
            self.motion.resets_remaining = (self.motion.resets_remaining - 1).max(0);
        }
    }

    fn advance_bounded_contact_field_step(&mut self) {
        let Some(kinematics) = self.dynamics_model().cloned() else {
            return;
        };
        let Some(contact_field) = kinematics.contact_field.as_ref() else {
            return;
        };
        let arena_w = kinematics.arena_width;
        let arena_h = kinematics.arena_height;
        let body_size = kinematics.body.size;
        let controller_w = kinematics.primary_control.width;
        let controller_h = kinematics.primary_control.height;
        let control_y = kinematics.primary_control.y;

        self.motion.body_x += self.motion.body_dx;
        self.motion.body_y += self.motion.body_dy;
        if self.motion.body_x <= 0 {
            self.motion.body_x = 0;
            self.motion.body_dx = self.motion.body_dx.abs();
        } else if self.motion.body_x + body_size >= arena_w {
            self.motion.body_x = arena_w - body_size;
            self.motion.body_dx = -self.motion.body_dx.abs();
        }
        if self.motion.body_y <= 0 {
            self.motion.body_y = 0;
            self.motion.body_dy = self.motion.body_dy.abs();
        }

        if self.motion.body_dy < 0 {
            let margin = contact_field.margin;
            let gap = contact_field.gap;
            let contact_h = contact_field.height;
            let rows = self.motion.contact_field_rows as i64;
            let cols = self.motion.contact_field_cols as i64;
            let contact_w = if cols > 0 {
                (arena_w - margin * 2 - gap * (cols - 1)) / cols
            } else {
                0
            };
            'contact_field_scan: for row in 0..rows {
                for col in 0..cols {
                    let idx = (row * cols + col) as usize;
                    if !self.motion.contact_field.get(idx).copied().unwrap_or(false) {
                        continue;
                    }
                    let bx = margin + col * (contact_w + gap);
                    let by = contact_field.top + row * (contact_h + gap);
                    if rects_overlap(
                        self.motion.body_x,
                        self.motion.body_y,
                        body_size,
                        body_size,
                        bx,
                        by,
                        contact_w,
                        contact_h,
                    ) {
                        self.motion.contact_field[idx] = false;
                        self.motion.body_dy = self.motion.body_dy.abs();
                        self.motion.contact_value += contact_field.value_per_contact;
                        break 'contact_field_scan;
                    }
                }
            }
        }

        let control_x = controller_left_from_position(self.motion.control_x, arena_w, controller_w);
        if self.motion.body_dy > 0
            && rects_overlap(
                self.motion.body_x,
                self.motion.body_y,
                body_size,
                body_size,
                control_x,
                control_y,
                controller_w,
                controller_h,
            )
        {
            self.motion.body_y = control_y - body_size;
            self.motion.body_dy = -self.motion.body_dy.abs();
            self.motion.body_dx = (self.motion.body_dx
                + ((self.motion.body_x + body_size / 2) - (control_x + controller_w / 2)) / 18)
                .clamp(-18, 18);
        }
        if self.motion.body_y > arena_h {
            self.motion.body_x = control_x + controller_w / 2 - body_size / 2;
            self.motion.body_y = control_y - body_size - 2;
            self.motion.body_dx = kinematics.body.dx;
            self.motion.body_dy = kinematics.body.dy;
            self.motion.resets_remaining = (self.motion.resets_remaining - 1).max(0);
        }
        if self.motion.contact_field.iter().all(|live| !*live) {
            self.motion.contact_field.fill(true);
        }
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
                RuntimeListVisibility::All => true,
                RuntimeListVisibility::Unmarked => !record.bool_field(LIST_MARK_FIELD),
                RuntimeListVisibility::Marked => record.bool_field(LIST_MARK_FIELD),
            })
    }

    fn render_matrix_text(&self) -> String {
        let mut lines = vec![
            self.program.title.clone(),
            "surface: dense_grid".to_string(),
            format!(
                "selected: {}{}",
                column_name(self.grid.selected.1),
                self.grid.selected.0
            ),
            format!(
                "expression: {}",
                self.dense_text(self.grid.selected.0, self.grid.selected.1)
            ),
            format!(
                "value: {}",
                self.dense_value(self.grid.selected.0, self.grid.selected.1)
            ),
            "columns: A B C D E F ... Z".to_string(),
        ];
        for row in 1..=self.grid.rows.min(5) {
            lines.push(format!(
                "row {row}: A={} | B={} | C={}",
                self.dense_value(row, 1.min(self.grid.columns)),
                self.dense_value(row, 2.min(self.grid.columns)),
                self.dense_value(row, 3.min(self.grid.columns))
            ));
        }
        lines.push(format!(
            "row {} and column {} reachable",
            self.grid.rows,
            column_name(self.grid.columns)
        ));
        lines.join("\n")
    }

    fn dense_value(&self, row: usize, col: usize) -> &str {
        &self.grid.value[self.dense_idx(row, col)]
    }

    fn dense_text(&self, row: usize, col: usize) -> &str {
        &self.grid.text[self.dense_idx(row, col)]
    }

    fn dense_idx(&self, row: usize, col: usize) -> usize {
        (row - 1) * self.grid.columns + (col - 1)
    }

    fn set_dense_text(&mut self, row: usize, col: usize, text: String) {
        let idx = self.dense_idx(row, col);
        for dep in self.grid.deps[idx].drain(..) {
            self.grid.rev_deps[dep].retain(|dependent| *dependent != idx);
        }
        let deps = self.collect_dense_expression_refs(&text);
        for dep in &deps {
            if !self.grid.rev_deps[*dep].contains(&idx) {
                self.grid.rev_deps[*dep].push(idx);
            }
        }
        self.grid.deps[idx] = deps;
        self.grid.text[idx] = text;
        self.recalc_dirty_dense(idx);
    }

    fn recalc_dirty_dense(&mut self, changed: usize) {
        let mut dirty = BTreeSet::new();
        self.collect_dense_dependents(changed, &mut dirty);
        let mut memo = BTreeMap::new();
        for idx in dirty {
            let value = self.evaluate_dense_slot(idx, &mut BTreeSet::new(), &mut memo);
            self.grid.value[idx] = value;
        }
    }

    fn collect_dense_dependents(&self, idx: usize, dirty: &mut BTreeSet<usize>) {
        if dirty.insert(idx) {
            for dependent in &self.grid.rev_deps[idx] {
                self.collect_dense_dependents(*dependent, dirty);
            }
        }
    }

    fn evaluate_dense_slot(
        &self,
        idx: usize,
        visiting: &mut BTreeSet<usize>,
        memo: &mut BTreeMap<usize, String>,
    ) -> String {
        if let Some(value) = memo.get(&idx) {
            return value.clone();
        }
        if !visiting.insert(idx) {
            return "#CYCLE".to_string();
        }
        let text = &self.grid.text[idx];
        let value = if let Some(expression) = text.strip_prefix('=') {
            self.resolve_dense_expression(expression, visiting, memo)
        } else {
            text.clone()
        };
        visiting.remove(&idx);
        memo.insert(idx, value.clone());
        value
    }

    fn resolve_dense_expression(
        &self,
        expression: &str,
        visiting: &mut BTreeSet<usize>,
        memo: &mut BTreeMap<usize, String>,
    ) -> String {
        if self.dense_expression_enabled("add")
            && let Some(args) = expression
                .strip_prefix("add(")
                .and_then(|rest| rest.strip_suffix(')'))
        {
            let parts = args.split(',').map(str::trim).collect::<Vec<_>>();
            if parts.len() != 2 {
                return "#ERR".to_string();
            }
            let Some(left) = self.decode_dense_ref(parts[0]) else {
                return "#ERR".to_string();
            };
            let Some(right) = self.decode_dense_ref(parts[1]) else {
                return "#ERR".to_string();
            };
            let Some(left) = self.evaluate_dense_number(left, visiting, memo) else {
                return "#CYCLE".to_string();
            };
            let Some(right) = self.evaluate_dense_number(right, visiting, memo) else {
                return "#CYCLE".to_string();
            };
            return (left + right).to_string();
        }
        if self.dense_expression_enabled("sum")
            && let Some(args) = expression
                .strip_prefix("sum(")
                .and_then(|rest| rest.strip_suffix(')'))
        {
            let Some((start, end)) = self.parse_dense_range(args.trim()) else {
                return "#ERR".to_string();
            };
            let mut sum = 0;
            for row in start.0.min(end.0)..=start.0.max(end.0) {
                for col in start.1.min(end.1)..=start.1.max(end.1) {
                    let Some(value) =
                        self.evaluate_dense_number(self.dense_idx(row, col), visiting, memo)
                    else {
                        return "#CYCLE".to_string();
                    };
                    sum += value;
                }
            }
            return sum.to_string();
        }
        "#ERR".to_string()
    }

    fn dense_expression_enabled(&self, name: &str) -> bool {
        self.app_ir.matrix_models.first().is_some_and(|grid| {
            grid.expression_functions
                .iter()
                .any(|function| function == name)
        })
    }

    fn collect_dense_expression_refs(&self, text: &str) -> Vec<usize> {
        let Some(expression) = text.strip_prefix('=') else {
            return Vec::new();
        };
        if self.dense_expression_enabled("add")
            && let Some(args) = expression
                .strip_prefix("add(")
                .and_then(|rest| rest.strip_suffix(')'))
        {
            return args
                .split(',')
                .filter_map(|arg| self.decode_dense_ref(arg.trim()))
                .collect();
        }
        if self.dense_expression_enabled("sum")
            && let Some(args) = expression
                .strip_prefix("sum(")
                .and_then(|rest| rest.strip_suffix(')'))
            && let Some((start, end)) = self.parse_dense_range(args.trim())
        {
            let mut deps = Vec::new();
            for row in start.0.min(end.0)..=start.0.max(end.0) {
                for col in start.1.min(end.1)..=start.1.max(end.1) {
                    deps.push(self.dense_idx(row, col));
                }
            }
            return deps;
        }
        Vec::new()
    }

    fn parse_dense_range(&self, text: &str) -> Option<((usize, usize), (usize, usize))> {
        let (start, end) = text.split_once(':')?;
        Some((
            self.decode_dense_ref_tuple(start)?,
            self.decode_dense_ref_tuple(end)?,
        ))
    }

    fn decode_dense_ref(&self, text: &str) -> Option<usize> {
        let (row, col) = self.decode_dense_ref_tuple(text)?;
        Some(self.dense_idx(row, col))
    }

    fn decode_dense_ref_tuple(&self, text: &str) -> Option<(usize, usize)> {
        let mut chars = text.chars();
        let col = chars.next()?.to_ascii_uppercase();
        if !col.is_ascii_uppercase() {
            return None;
        }
        let row = chars.as_str().parse::<usize>().ok()?;
        let col = (col as u8).checked_sub(b'A')? as usize + 1;
        if row == 0 || row > self.grid.rows || col == 0 || col > self.grid.columns {
            return None;
        }
        Some((row, col))
    }

    fn parse_dense_owner(&self, owner_id: &str) -> Result<(usize, usize)> {
        self.decode_dense_ref_tuple(owner_id)
            .ok_or_else(|| anyhow::anyhow!("dense owner_id `{owner_id}` is outside compiled grid"))
    }

    fn evaluate_dense_number(
        &self,
        idx: usize,
        visiting: &mut BTreeSet<usize>,
        memo: &mut BTreeMap<usize, String>,
    ) -> Option<i64> {
        let value = self.evaluate_dense_slot(idx, visiting, memo);
        if value == "#CYCLE" {
            None
        } else {
            Some(value.parse::<i64>().unwrap_or(0))
        }
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
            self.parse_dense_owner(owner_id)?;
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
                let record = self
                    .dynamic_values
                    .iter_mut()
                    .find(|record| record.id == dynamic_value_id)
                    .ok_or_else(|| {
                        anyhow::anyhow!("dynamic value owner `{owner_id}` is not live")
                    })?;
                record.set_text_field(LIST_TEXT_FIELD, value);
                record.set_focus_field(LIST_EDIT_FOCUS, true);
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
                    .map(|owner_id| self.parse_dense_owner(owner_id))
                    .transpose()?
                    .unwrap_or(self.grid.selected);
                self.set_dense_text(row, col, value);
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
                if let Some(record) = self
                    .dynamic_values
                    .iter_mut()
                    .find(|record| record.id == dynamic_value_id)
                    && matches!(event.value, SourceValue::Tag(ref key) if key == "Enter")
                {
                    record.set_focus_field(LIST_EDIT_FOCUS, false);
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
                if let Some(record) = self
                    .dynamic_values
                    .iter_mut()
                    .find(|record| record.id == dynamic_value_id)
                    && event.path.ends_with(".event.blur")
                {
                    record.set_focus_field(LIST_EDIT_FOCUS, false);
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
                    .map(|owner_id| self.parse_dense_owner(owner_id))
                    .transpose()?
                    .unwrap_or(self.grid.selected);
                self.grid.selected = (row, col);
                self.grid.edit_focus = Some((row, col));
                results.push(self.emit_frame_owned(self.dense_change_paths(), metrics));
            } else if self
                .wiring
                .grid
                .as_ref()
                .and_then(|grid| grid.editor_key.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                if matches!(event.value, SourceValue::Tag(ref key) if key == "Enter") {
                    self.grid.edit_focus = None;
                }
                results.push(self.emit_frame_owned(self.dense_change_paths(), metrics));
            } else if self
                .wiring
                .grid
                .as_ref()
                .and_then(|grid| grid.viewport_key.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                if let SourceValue::Tag(key) = &event.value {
                    match key.as_str() {
                        "ArrowUp" => {
                            self.grid.selected.0 = self.grid.selected.0.saturating_sub(1).max(1);
                        }
                        "ArrowDown" => {
                            self.grid.selected.0 = (self.grid.selected.0 + 1).min(self.grid.rows);
                        }
                        "ArrowLeft" => {
                            self.grid.selected.1 = self.grid.selected.1.saturating_sub(1).max(1);
                        }
                        "ArrowRight" => {
                            self.grid.selected.1 =
                                (self.grid.selected.1 + 1).min(self.grid.columns);
                        }
                        _ => {}
                    }
                }
                results.push(self.emit_frame_owned(self.dense_selection_change_paths(), metrics));
            } else if self
                .wiring
                .kinematic_control_event
                .as_ref()
                .is_some_and(|path| event.path == *path)
            {
                if let SourceValue::Tag(key) = &event.value
                    && let Some(kinematics) = self.dynamics_model().cloned()
                {
                    match kinematics.primary_control.axis {
                        ControlAxis::Horizontal => match key.as_str() {
                            "ArrowLeft" | "ArrowUp" => {
                                self.motion.control_x = (self.motion.control_x
                                    - kinematics.primary_control.step)
                                    .max(0);
                            }
                            "ArrowRight" | "ArrowDown" => {
                                self.motion.control_x = (self.motion.control_x
                                    + kinematics.primary_control.step)
                                    .min(100);
                            }
                            _ => {}
                        },
                        ControlAxis::Vertical => match key.as_str() {
                            "ArrowUp" | "ArrowLeft" => {
                                self.motion.control_y = (self.motion.control_y
                                    - kinematics.primary_control.step)
                                    .max(0);
                            }
                            "ArrowDown" | "ArrowRight" => {
                                self.motion.control_y = (self.motion.control_y
                                    + kinematics.primary_control.step)
                                    .min(100);
                            }
                            _ => {}
                        },
                    }
                }
                results.push(self.emit_frame(&["kinematics.control"], metrics));
            } else if self
                .wiring
                .kinematic_frame_event
                .as_ref()
                .is_some_and(|path| event.path == *path)
            {
                self.advance_kinematic_step();
                results.push(self.emit_frame(&["frame", "kinematics.body"], metrics));
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
            .filter(|record| record.bool_field(LIST_MARK_FIELD))
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
                        .map(|record| record.text_field(LIST_TEXT_FIELD))
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
                    json!(record.text_field(LIST_TEXT_FIELD)),
                );
                values.insert(
                    format!("store.{}[{}].mark", sequence.root, record.id),
                    json!(record.bool_field(LIST_MARK_FIELD)),
                );
                values.insert(
                    format!("store.{}[{}].edit_focus", sequence.root, record.id),
                    json!(record.focus_field(LIST_EDIT_FOCUS)),
                );
            }
        }
        values.insert("kinematics.frame".to_string(), json!(self.frame_index));
        values.insert(
            "kinematics.control_y".to_string(),
            json!(self.motion.control_y),
        );
        values.insert(
            "kinematics.control_x".to_string(),
            json!(self.motion.control_x),
        );
        values.insert(
            "kinematics.tracked_control_y".to_string(),
            json!(self.motion.tracked_control_y),
        );
        values.insert("kinematics.body_x".to_string(), json!(self.motion.body_x));
        values.insert("kinematics.body_y".to_string(), json!(self.motion.body_y));
        values.insert("kinematics.body_dx".to_string(), json!(self.motion.body_dx));
        values.insert("kinematics.body_dy".to_string(), json!(self.motion.body_dy));
        values.insert(
            "kinematics.contact_field_rows".to_string(),
            json!(self.motion.contact_field_rows as i64),
        );
        values.insert(
            "kinematics.contact_field_cols".to_string(),
            json!(self.motion.contact_field_cols as i64),
        );
        values.insert(
            "kinematics.contact_field_live_count".to_string(),
            json!(
                self.motion
                    .contact_field
                    .iter()
                    .filter(|live| **live)
                    .count() as i64
            ),
        );
        values.insert(
            "kinematics.contact_value".to_string(),
            json!(self.motion.contact_value),
        );
        values.insert(
            "kinematics.resets_remaining".to_string(),
            json!(self.motion.resets_remaining),
        );
        let grid_root = self
            .wiring
            .grid
            .as_ref()
            .map(|grid| grid.root.as_str())
            .unwrap_or("grid");
        if self.wiring.grid.is_some() {
            for row in 1..=self.grid.rows {
                for col in 1..=self.grid.columns {
                    let coordinate = format!("{}{}", column_name(col), row);
                    values.insert(
                        format!("{grid_root}.{coordinate}"),
                        json!(self.dense_value(row, col)),
                    );
                    values.insert(
                        format!("{grid_root}.{coordinate}.expression"),
                        json!(self.dense_text(row, col)),
                    );
                }
            }
        }
        values.insert(
            format!("{grid_root}.selected_expression"),
            json!(self.dense_text(self.grid.selected.0, self.grid.selected.1)),
        );
        values.insert(
            format!("{grid_root}.selected_value"),
            json!(self.dense_value(self.grid.selected.0, self.grid.selected.1)),
        );
        values.insert(
            format!("{grid_root}.selected"),
            json!(format!(
                "{}{}",
                column_name(self.grid.selected.1),
                self.grid.selected.0
            )),
        );
        values.insert(
            format!("{grid_root}.edit_focus"),
            json!(
                self.grid
                    .edit_focus
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

#[allow(clippy::too_many_arguments)]
fn push_hit_target(
    scene: &mut FrameScene,
    id: impl Into<String>,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    action: HitTargetAction,
    source_path: impl Into<String>,
) {
    scene.hit_targets.push(HitTarget {
        id: id.into(),
        x,
        y,
        width,
        height,
        action,
        source_path: source_path.into(),
        owner_id: None,
        generation: 0,
        text_state_path: None,
        text_value: None,
        key_event_path: None,
        change_event_path: None,
        focus_event_path: None,
        blur_event_path: None,
    });
}

#[allow(clippy::too_many_arguments)]
fn push_text_hit_target(
    scene: &mut FrameScene,
    id: impl Into<String>,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    source_path: impl Into<String>,
    text_state_path: impl Into<String>,
    key_event_path: Option<String>,
    change_event_path: Option<String>,
    focus_event_path: Option<String>,
    blur_event_path: Option<String>,
) {
    scene.hit_targets.push(HitTarget {
        id: id.into(),
        x,
        y,
        width,
        height,
        action: HitTargetAction::FocusText,
        source_path: source_path.into(),
        owner_id: None,
        generation: 0,
        text_state_path: Some(text_state_path.into()),
        text_value: None,
        key_event_path,
        change_event_path,
        focus_event_path,
        blur_event_path,
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

fn position_from_controller_top(top: i64, arena_h: i64, controller_h: i64) -> i64 {
    let span = (arena_h - controller_h).max(1);
    (top.clamp(0, span) * 100 / span).clamp(0, 100)
}

fn ranges_overlap(a0: i64, a1: i64, b0: i64, b1: i64) -> bool {
    a0 < b1 && b0 < a1
}

#[allow(clippy::too_many_arguments)]
fn rects_overlap(ax: i64, ay: i64, aw: i64, ah: i64, bx: i64, by: i64, bw: i64, bh: i64) -> bool {
    ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
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
