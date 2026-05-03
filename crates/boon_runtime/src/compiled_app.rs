use crate::{
    AppSnapshot, BoonApp, RuntimeClock, SourceBatch, SourceEmission, SourceInventory, SourceValue,
    StateDelta, TurnId, TurnMetrics, TurnResult,
};
use anyhow::{Result, bail};
use boon_compiler::{
    AppIr, ControlAxis, IrEffect, IrPredicate, IrValueExpr, ProgramSpec, RecordVisibility,
    SequenceViewSpec, SurfaceKind,
};
use boon_render_ir::{
    DrawCommand, FrameScene, HitTarget, HitTargetAction, HostPatch, NodeId, NodeKind,
};
use boon_shape::Shape;
use boon_source::{SourceEntry, SourceOwner};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct CompiledApp {
    program: ProgramSpec,
    app_ir: AppIr,
    inventory: SourceInventory,
    wiring: RuntimeWiring,
    turn: u64,
    frame_text: String,
    scalar_value: i64,
    clock_value: i64,
    clock: RuntimeClock,
    records: Vec<DynamicRecord>,
    next_record_id: u64,
    entry_text: String,
    source_state: BTreeMap<String, SourceValue>,
    generic_state: BTreeMap<String, i64>,
    view_selector: String,
    grid: DenseGridState,
    frame_index: u64,
    kinematics: KinematicState,
}

#[derive(Clone, Debug, Default)]
struct RuntimeWiring {
    scalar_event: Option<String>,
    sequence: Option<SequenceBinding>,
    grid: Option<DenseBinding>,
    kinematic_frame_event: Option<String>,
    kinematic_control_event: Option<String>,
}

#[derive(Clone, Debug)]
struct SequenceBinding {
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
    fn from_compiled(program: &ProgramSpec, app_ir: &AppIr, inventory: &SourceInventory) -> Self {
        let scalar_event = program
            .scalar_accumulator
            .as_ref()
            .map(|scalar_value| scalar_value.event_path.clone());
        let sequence = SequenceBinding::from_app_ir(inventory, app_ir);
        let grid = program
            .dense_grid
            .as_ref()
            .and_then(|_| DenseBinding::from_inventory(inventory));
        let kinematic_frame_event = program
            .kinematics
            .as_ref()
            .map(|kinematics| kinematics.frame_event_path.clone());
        let kinematic_control_event = program
            .kinematics
            .as_ref()
            .map(|kinematics| kinematics.control_event_path.clone());
        Self {
            scalar_event,
            sequence,
            grid,
            kinematic_frame_event,
            kinematic_control_event,
        }
    }
}

impl SequenceBinding {
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
        let family = first_dynamic_family(inventory, "Element/checkbox(element.event.click)")
            .or_else(|| first_dynamic_family(inventory, "Element/text_input(element.text)"))?;
        let root = dynamic_family_root(&family);
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
        let dynamic_text_base =
            dynamic_base_for_producer(inventory, &family, "Element/text_input(element.text)");
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

fn eval_initial_number(expr: &IrValueExpr) -> Result<i64> {
    match expr {
        IrValueExpr::Number { value } => Ok(*value),
        _ => bail!("generic state cell initial value must be a number"),
    }
}

fn unique_paths(paths: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct DynamicRecord {
    id: u64,
    generation: u32,
    content_text: String,
    mark: bool,
    edit_focus: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DenseGridState {
    rows: usize,
    columns: usize,
    selected: (usize, usize),
    edit_focus: Option<(usize, usize)>,
    text: Vec<String>,
    value: Vec<String>,
    deps: Vec<Vec<usize>>,
    rev_deps: Vec<Vec<usize>>,
}

impl DenseGridState {
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
struct KinematicState {
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

impl KinematicState {
    fn from_spec(spec: Option<&boon_compiler::KinematicSpec>) -> Self {
        let Some(spec) = spec else {
            return Self::default();
        };
        let contact_field_rows = spec
            .contact_field
            .as_ref()
            .map_or(0, |contact_field| contact_field.rows);
        let contact_field_cols = spec
            .contact_field
            .as_ref()
            .map_or(0, |contact_field| contact_field.columns);
        Self {
            body_x: spec.body.x,
            body_y: spec.body.y,
            body_dx: spec.body.dx,
            body_dy: spec.body.dy,
            control_x: if matches!(spec.primary_control.axis, ControlAxis::Horizontal) {
                spec.primary_control.position
            } else {
                50
            },
            control_y: if matches!(spec.primary_control.axis, ControlAxis::Vertical) {
                spec.primary_control.position
            } else {
                50
            },
            tracked_control_y: spec
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

impl Default for KinematicState {
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
        let wiring = RuntimeWiring::from_compiled(&program, &app_ir, &inventory);
        let initial_records = app_ir
            .list_states
            .first()
            .map(|list| list.initial_items.clone())
            .unwrap_or_default();
        let grid = program
            .dense_grid
            .as_ref()
            .map(|grid| DenseGridState::new(grid.rows, grid.columns))
            .unwrap_or_else(|| DenseGridState::new(100, 26));
        let kinematics = KinematicState::from_spec(program.kinematics.as_ref());
        let initial_view_selector = program
            .sequence
            .as_ref()
            .and_then(|sequence| {
                sequence
                    .view_selectors
                    .first()
                    .map(|selector| selector.id.clone())
            })
            .unwrap_or_else(|| "all".to_string());
        let mut generic_state = BTreeMap::new();
        for cell in &app_ir.state_cells {
            if let Ok(value) = eval_initial_number(&cell.initial) {
                generic_state.insert(cell.path.clone(), value);
            }
        }
        let mut app = Self {
            program,
            app_ir,
            inventory,
            wiring,
            turn: 0,
            frame_text: String::new(),
            scalar_value: *generic_state.get("scalar_value").unwrap_or(&0),
            clock_value: *generic_state.get("clock_value").unwrap_or(&0),
            clock: RuntimeClock::default(),
            records: initial_records
                .into_iter()
                .enumerate()
                .map(|(idx, item)| DynamicRecord {
                    id: idx as u64 + 1,
                    generation: 0,
                    content_text: item.text.unwrap_or_default(),
                    mark: item.mark,
                    edit_focus: false,
                })
                .collect(),
            next_record_id: 1,
            entry_text: String::new(),
            source_state: BTreeMap::new(),
            generic_state,
            view_selector: initial_view_selector,
            grid,
            frame_index: 0,
            kinematics,
        };
        app.next_record_id = app.records.len() as u64 + 1;
        app.frame_text = app.render_text();
        app
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
            .sequence
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
                    .sequence
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
            .sequence
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
        self.wiring.sequence.as_ref().is_some_and(|sequence| {
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
        self.wiring.sequence.as_ref().is_some_and(|sequence| {
            [&sequence.dynamic_text_blur, &sequence.dynamic_text_change]
                .into_iter()
                .flatten()
                .any(|candidate| candidate == path)
        })
    }

    fn apply_generic_event(&mut self, event: &SourceEmission) -> Result<Option<Vec<String>>> {
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

    fn set_generic_number(&mut self, state_path: &str, value: i64) {
        self.generic_state.insert(state_path.to_string(), value);
        match state_path {
            "scalar_value" => self.scalar_value = value,
            "clock_value" => self.clock_value = value,
            _ => {}
        }
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
            .sequence
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
        self.records.push(DynamicRecord {
            id: self.next_record_id,
            generation: 0,
            content_text: text,
            mark: false,
            edit_focus: false,
        });
        self.next_record_id += 1;
        Ok(true)
    }

    fn apply_generic_list_mark_all(&mut self, list_path: &str) -> bool {
        if self.record_root() != Some(list_path) {
            return false;
        }
        let all_marked = self.records.iter().all(|record| record.mark);
        for record in &mut self.records {
            record.mark = !all_marked;
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
        let record_id = owner_id
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("record owner_id `{owner_id}` is not numeric"))?;
        let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.id == record_id)
        else {
            return Ok(false);
        };
        record.mark = !record.mark;
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
        let record_id = owner_id
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("record owner_id `{owner_id}` is not numeric"))?;
        let before = self.records.len();
        self.records.retain(|record| record.id != record_id);
        Ok(self.records.len() != before)
    }

    fn apply_generic_list_remove_marked(&mut self, list_path: &str) -> bool {
        if self.record_root() != Some(list_path) {
            return false;
        }
        let before = self.records.len();
        self.records.retain(|record| !record.mark);
        self.records.len() != before
    }

    fn clear_generic_text_state(&mut self, text_state_path: &str) {
        if self
            .wiring
            .sequence
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

    fn sequence_view(&self) -> Option<&SequenceViewSpec> {
        self.program
            .sequence
            .as_ref()
            .map(|sequence| &sequence.view)
    }

    fn render_text(&self) -> String {
        match self.program.scene {
            SurfaceKind::Sequence => self.render_record_sequence_text(),
            SurfaceKind::DenseGrid => self.render_dense_plane_text(),
            SurfaceKind::ActionValue => {
                let label = self
                    .program
                    .scalar_accumulator
                    .as_ref()
                    .map(|scalar_value| scalar_value.button_label.as_str())
                    .filter(|label| !label.is_empty())
                    .unwrap_or("action");
                format!(
                    "{}\nsurface: button-scalar\n[ {label} ]\ncount: {}",
                    self.program.title, self.scalar_value
                )
            }
            SurfaceKind::ClockValue => format!(
                "{}\nsurface: clock-scalar\nruntime_clock_ms: {}\nticks: {}",
                self.program.title, self.clock.millis, self.clock_value
            ),
            SurfaceKind::Kinematics => self.render_kinematic_text(),
            SurfaceKind::Blank => String::new(),
        }
    }

    fn render_scene(&self) -> FrameScene {
        let mut scene = FrameScene {
            title: self.program.title.clone(),
            commands: Vec::new(),
            hit_targets: Vec::new(),
        };
        push_rect(&mut scene, 0, 0, 1000, 1000, [245, 245, 245, 255]);
        match self.program.scene {
            SurfaceKind::Sequence => self.render_record_sequence_scene(&mut scene),
            SurfaceKind::DenseGrid => self.render_dense_grid_scene(&mut scene),
            SurfaceKind::ActionValue => self.render_action_value_scene(&mut scene),
            SurfaceKind::ClockValue => self.render_clock_value_scene(&mut scene),
            SurfaceKind::Kinematics => self.render_kinematic_scene(&mut scene),
            SurfaceKind::Blank => {}
        }
        scene
    }

    fn render_action_value_scene(&self, scene: &mut FrameScene) {
        push_rect(scene, 0, 0, 1000, 1000, [238, 244, 247, 255]);
        push_text(scene, 84, 108, 3, &self.program.title, [25, 40, 52, 255]);
        push_rect(scene, 338, 388, 324, 92, [46, 125, 166, 255]);
        push_rect_outline(scene, 338, 388, 324, 92, [21, 91, 128, 255]);
        if let Some(path) = self.wiring.scalar_event.as_deref() {
            push_hit_target(
                scene,
                "scalar_action",
                338,
                388,
                324,
                92,
                HitTargetAction::Press,
                path,
            );
        }
        let label = self
            .program
            .scalar_accumulator
            .as_ref()
            .map(|scalar_value| scalar_value.button_label.as_str())
            .filter(|label| !label.is_empty())
            .unwrap_or("action");
        push_text(scene, 424, 424, 2, label, [255, 255, 255, 255]);
        push_text(
            scene,
            424,
            548,
            3,
            &format!("count {}", self.scalar_value),
            [35, 55, 68, 255],
        );
    }

    fn render_clock_value_scene(&self, scene: &mut FrameScene) {
        push_rect(scene, 0, 0, 1000, 1000, [235, 241, 245, 255]);
        push_text(scene, 84, 108, 3, &self.program.title, [24, 42, 55, 255]);
        push_rect(scene, 120, 280, 760, 270, [28, 44, 58, 255]);
        push_rect_outline(scene, 120, 280, 760, 270, [91, 156, 187, 255]);
        push_text(
            scene,
            184,
            348,
            4,
            &format!("ticks {}", self.clock_value),
            [240, 250, 255, 255],
        );
        push_text(
            scene,
            184,
            456,
            2,
            &format!("runtime clock {} ms", self.clock.millis),
            [166, 207, 224, 255],
        );
    }

    fn render_record_sequence_scene(&self, scene: &mut FrameScene) {
        let view = self.sequence_view();
        push_rect(scene, 0, 0, 1000, 1000, [245, 245, 245, 255]);
        push_text(
            scene,
            340,
            66,
            4,
            view.map(|view| view.title_line.as_str())
                .filter(|title_line| !title_line.is_empty())
                .unwrap_or(&self.program.title),
            [186, 137, 137, 255],
        );
        push_rect(scene, 206, 160, 588, 72, [255, 255, 255, 255]);
        push_rect_outline(scene, 206, 160, 588, 72, [225, 225, 225, 255]);
        push_rect_outline(scene, 226, 184, 28, 28, [198, 198, 198, 255]);
        if let Some(sequence) = &self.wiring.sequence {
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
                view.map(|view| view.entry_hint.as_str())
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
        let visible_records: Vec<_> = self.visible_records().collect();
        let mut y = 234;
        for record in &visible_records {
            if y >= 1000 {
                break;
            }
            push_rect(scene, 206, y, 588, 62, [255, 255, 255, 255]);
            push_rect_outline(scene, 206, y, 588, 62, [232, 232, 232, 255]);
            push_rect_outline(scene, 226, y + 18, 24, 24, [126, 178, 164, 255]);
            if let Some(sequence) = &self.wiring.sequence {
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
                            text_value: Some(record.content_text.clone()),
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
            if record.mark {
                push_text(scene, 231, y + 19, 1, "x", [68, 146, 126, 255]);
            }
            push_text(
                scene,
                270,
                y + 22,
                2,
                &record.content_text,
                if record.mark {
                    [160, 160, 160, 255]
                } else {
                    [60, 60, 60, 255]
                },
            );
            push_text(scene, 744, y + 22, 1, "x", [172, 84, 84, 255]);
            y += 62;
        }
        let mark = self.records.iter().filter(|record| record.mark).count();
        let unmarked = self.records.len().saturating_sub(mark);
        y = 234 + visible_records.len() as u32 * 62;
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
                view.map(|view| view.count_suffix.as_str())
                    .unwrap_or("unmarked")
            ),
            [116, 116, 116, 255],
        );
        let view_selectors = self.sequence_view_selector_layout();
        for (view_selector, x, outline_w, label) in view_selectors {
            if self.view_selector == view_selector {
                push_rect_outline(scene, x - 10, y + 12, outline_w, 28, [218, 185, 185, 255]);
            }
            if let Some(path) = self
                .wiring
                .sequence
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
            && let Some(label) = view.and_then(|view| view.remove_marked_label.as_deref())
        {
            if let Some(path) = self
                .wiring
                .sequence
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

    fn sequence_view_selector_layout(&self) -> Vec<(String, u32, u32, String)> {
        let view_selectors = self
            .program
            .sequence
            .as_ref()
            .map(|sequence| sequence.view_selectors.clone())
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

    fn render_dense_grid_scene(&self, scene: &mut FrameScene) {
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
        push_rect(scene, origin_x, origin_y, 904, 40, [229, 235, 241, 255]);
        push_rect(scene, origin_x, origin_y, 52, 760, [229, 235, 241, 255]);
        for col in 1..=9 {
            let x = origin_x + 52 + (col as u32 - 1) * col_w;
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
        for row in 1..=15 {
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
            for col in 1..=9 {
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

    fn render_kinematic_scene(&self, scene: &mut FrameScene) {
        let Some(kinematics) = self.program.kinematics.as_ref() else {
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
                self.frame_index, self.kinematics.contact_value, self.kinematics.resets_remaining
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
                    if self
                        .kinematics
                        .contact_field
                        .get(idx)
                        .copied()
                        .unwrap_or(false)
                    {
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
                self.kinematics.control_x,
                kinematics.arena_width,
                kinematics.primary_control.width,
            )
        } else {
            kinematics.primary_control.x
        };
        let primary_y = if kinematics.primary_control.axis == ControlAxis::Vertical {
            controller_top_from_position(
                self.kinematics.control_y,
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
                self.kinematics.tracked_control_y,
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
            sx(self.kinematics.body_x),
            sy(self.kinematics.body_y),
            sw(kinematics.body.size),
            sh(kinematics.body.size),
            [250, 250, 250, 255],
        );
    }

    fn render_record_sequence_text(&self) -> String {
        let view = self.sequence_view();
        let mark = self.records.iter().filter(|record| record.mark).count();
        let unmarked = self.records.len().saturating_sub(mark);
        let mut lines = vec![
            self.program.title.clone(),
            "surface: sequence".to_string(),
            view.map(|view| view.entry_hint.clone()).unwrap_or_default(),
            format!("input: {}", self.entry_text),
        ];
        for record in self.visible_records() {
            lines.push(format!(
                "{} [{}] {}",
                record.id,
                if record.mark { "x" } else { " " },
                record.content_text
            ));
        }
        lines.push(format!(
            "{unmarked} {}",
            view.map(|view| view.count_suffix.as_str())
                .unwrap_or("unmarked")
        ));
        lines.push(format!("view_selector: {}", self.view_selector));
        if let Some(view) = view {
            lines.extend(view.auxiliary_lines.iter().cloned());
        }
        lines.join("\n")
    }

    fn render_kinematic_text(&self) -> String {
        let frame_source = self
            .wiring
            .kinematic_frame_event
            .as_deref()
            .unwrap_or("frame source");
        format!(
            "{}\nsurface: kinematics\nkinematic_mode: {}\nframe: {}\ncontrol_y: {}\ncontrol_x: {}\ntracked_control_y: {}\nbody_x: {}\nbody_y: {}\nbody_dx: {}\nbody_dy: {}\ncontact_field_rows: {}\ncontact_field_cols: {}\ncontact_field_live: {}\ncontact_value: {}\nresets_remaining: {}\ndeterministic input source: {}",
            self.program.title,
            if self
                .program
                .kinematics
                .as_ref()
                .and_then(|kinematics| kinematics.contact_field.as_ref())
                .is_some()
            {
                "contact-field"
            } else {
                "dual-walls"
            },
            self.frame_index,
            self.kinematics.control_y,
            self.kinematics.control_x,
            self.kinematics.tracked_control_y,
            self.kinematics.body_x,
            self.kinematics.body_y,
            self.kinematics.body_dx,
            self.kinematics.body_dy,
            self.kinematics.contact_field_rows,
            self.kinematics.contact_field_cols,
            self.kinematics.live_contact_field_indices(),
            self.kinematics.contact_value,
            self.kinematics.resets_remaining,
            frame_source
        )
    }

    fn advance_kinematic_step(&mut self) {
        self.frame_index += 1;
        if self
            .program
            .kinematics
            .as_ref()
            .and_then(|kinematics| kinematics.contact_field.as_ref())
            .is_some()
        {
            self.advance_bounded_contact_field_step();
        } else {
            self.advance_bounded_peer_step();
        }
    }

    fn advance_bounded_peer_step(&mut self) {
        let Some(kinematics) = self.program.kinematics.as_ref() else {
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

        self.kinematics.body_x += self.kinematics.body_dx;
        self.kinematics.body_y += self.kinematics.body_dy;
        if self.kinematics.body_y <= 0 {
            self.kinematics.body_y = 0;
            self.kinematics.body_dy = self.kinematics.body_dy.abs();
        } else if self.kinematics.body_y + body_size >= arena_h {
            self.kinematics.body_y = arena_h - body_size;
            self.kinematics.body_dy = -self.kinematics.body_dy.abs();
        }

        self.kinematics.tracked_control_y = position_from_controller_top(
            self.kinematics.body_y + body_size / 2 - controller_h / 2,
            arena_h,
            controller_h,
        );
        let left_y = controller_top_from_position(self.kinematics.control_y, arena_h, controller_h);
        let right_y =
            controller_top_from_position(self.kinematics.tracked_control_y, arena_h, controller_h);

        if self.kinematics.body_dx < 0
            && self.kinematics.body_x <= left_x + controller_w
            && self.kinematics.body_x + body_size >= left_x
            && ranges_overlap(
                self.kinematics.body_y,
                self.kinematics.body_y + body_size,
                left_y,
                left_y + controller_h,
            )
        {
            self.kinematics.body_x = left_x + controller_w;
            self.kinematics.body_dx = self.kinematics.body_dx.abs();
            self.kinematics.body_dy = (self.kinematics.body_dy
                + ((self.kinematics.body_y + body_size / 2) - (left_y + controller_h / 2)) / 18)
                .clamp(-18, 18);
            self.kinematics.contact_value += 1;
        }
        if self.kinematics.body_dx > 0
            && self.kinematics.body_x + body_size >= right_x
            && self.kinematics.body_x <= right_x + controller_w
            && ranges_overlap(
                self.kinematics.body_y,
                self.kinematics.body_y + body_size,
                right_y,
                right_y + controller_h,
            )
        {
            self.kinematics.body_x = right_x - body_size;
            self.kinematics.body_dx = -self.kinematics.body_dx.abs();
            self.kinematics.body_dy = (self.kinematics.body_dy
                + ((self.kinematics.body_y + body_size / 2) - (right_y + controller_h / 2)) / 18)
                .clamp(-18, 18);
            self.kinematics.contact_value += 1;
        }
        if self.kinematics.body_x < -body_size || self.kinematics.body_x > arena_w + body_size {
            self.kinematics.body_x = arena_w / 2;
            self.kinematics.body_y = arena_h / 2;
            self.kinematics.body_dx = if self.kinematics.body_dx < 0 { 12 } else { -12 };
            self.kinematics.body_dy = 8;
            self.kinematics.resets_remaining = (self.kinematics.resets_remaining - 1).max(0);
        }
    }

    fn advance_bounded_contact_field_step(&mut self) {
        let Some(kinematics) = self.program.kinematics.as_ref() else {
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

        self.kinematics.body_x += self.kinematics.body_dx;
        self.kinematics.body_y += self.kinematics.body_dy;
        if self.kinematics.body_x <= 0 {
            self.kinematics.body_x = 0;
            self.kinematics.body_dx = self.kinematics.body_dx.abs();
        } else if self.kinematics.body_x + body_size >= arena_w {
            self.kinematics.body_x = arena_w - body_size;
            self.kinematics.body_dx = -self.kinematics.body_dx.abs();
        }
        if self.kinematics.body_y <= 0 {
            self.kinematics.body_y = 0;
            self.kinematics.body_dy = self.kinematics.body_dy.abs();
        }

        if self.kinematics.body_dy < 0 {
            let margin = contact_field.margin;
            let gap = contact_field.gap;
            let contact_h = contact_field.height;
            let rows = self.kinematics.contact_field_rows as i64;
            let cols = self.kinematics.contact_field_cols as i64;
            let contact_w = if cols > 0 {
                (arena_w - margin * 2 - gap * (cols - 1)) / cols
            } else {
                0
            };
            'contact_field_scan: for row in 0..rows {
                for col in 0..cols {
                    let idx = (row * cols + col) as usize;
                    if !self
                        .kinematics
                        .contact_field
                        .get(idx)
                        .copied()
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let bx = margin + col * (contact_w + gap);
                    let by = contact_field.top + row * (contact_h + gap);
                    if rects_overlap(
                        self.kinematics.body_x,
                        self.kinematics.body_y,
                        body_size,
                        body_size,
                        bx,
                        by,
                        contact_w,
                        contact_h,
                    ) {
                        self.kinematics.contact_field[idx] = false;
                        self.kinematics.body_dy = self.kinematics.body_dy.abs();
                        self.kinematics.contact_value += contact_field.value_per_contact;
                        break 'contact_field_scan;
                    }
                }
            }
        }

        let control_x =
            controller_left_from_position(self.kinematics.control_x, arena_w, controller_w);
        if self.kinematics.body_dy > 0
            && rects_overlap(
                self.kinematics.body_x,
                self.kinematics.body_y,
                body_size,
                body_size,
                control_x,
                control_y,
                controller_w,
                controller_h,
            )
        {
            self.kinematics.body_y = control_y - body_size;
            self.kinematics.body_dy = -self.kinematics.body_dy.abs();
            self.kinematics.body_dx = (self.kinematics.body_dx
                + ((self.kinematics.body_x + body_size / 2) - (control_x + controller_w / 2)) / 18)
                .clamp(-18, 18);
        }
        if self.kinematics.body_y > arena_h {
            self.kinematics.body_x = control_x + controller_w / 2 - body_size / 2;
            self.kinematics.body_y = control_y - body_size - 2;
            self.kinematics.body_dx = kinematics.body.dx;
            self.kinematics.body_dy = kinematics.body.dy;
            self.kinematics.resets_remaining = (self.kinematics.resets_remaining - 1).max(0);
        }
        if self.kinematics.contact_field.iter().all(|live| !*live) {
            self.kinematics.contact_field.fill(true);
        }
    }

    fn visible_records(&self) -> impl Iterator<Item = &DynamicRecord> {
        let visibility = self
            .program
            .sequence
            .as_ref()
            .and_then(|sequence| {
                sequence
                    .view_selectors
                    .iter()
                    .find(|selector| selector.id == self.view_selector)
                    .map(|selector| &selector.visibility)
            })
            .unwrap_or(&RecordVisibility::All);
        self.records.iter().filter(move |record| match visibility {
            RecordVisibility::All => true,
            RecordVisibility::Unmarked => !record.mark,
            RecordVisibility::Marked => record.mark,
        })
    }

    fn render_dense_plane_text(&self) -> String {
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
        self.program.dense_grid.as_ref().is_some_and(|grid| {
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
        if let Some(sequence) = &self.wiring.sequence
            && path.starts_with(&sequence.family)
        {
            let record_id = owner_id
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("record owner_id `{owner_id}` is not numeric"))?;
            return self
                .records
                .iter()
                .find(|record| record.id == record_id)
                .map(|record| record.generation)
                .ok_or_else(|| anyhow::anyhow!("dynamic record owner `{owner_id}` is not live"));
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
                .sequence
                .as_ref()
                .and_then(|sequence| sequence.entry_text.as_ref())
                .is_some_and(|path| update.path == *path)
            {
                if let SourceValue::Text(value) = update.value {
                    self.entry_text = value;
                }
            } else if self
                .wiring
                .sequence
                .as_ref()
                .and_then(|sequence| sequence.dynamic_text_value.as_ref())
                .is_some_and(|path| update.path == *path)
                && let SourceValue::Text(value) = update.value
            {
                let owner_id = update
                    .owner_id
                    .as_deref()
                    .expect("dynamic edit text owner_id was validated");
                let record_id = owner_id
                    .parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("record owner_id `{owner_id}` is not numeric"))?;
                let record = self
                    .records
                    .iter_mut()
                    .find(|record| record.id == record_id)
                    .ok_or_else(|| anyhow::anyhow!("record owner `{owner_id}` is not live"))?;
                record.content_text = value;
                record.edit_focus = true;
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
                .sequence
                .as_ref()
                .and_then(|sequence| sequence.dynamic_text_key.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                let owner_id = event
                    .owner_id
                    .as_deref()
                    .expect("dynamic edit_input key owner_id was validated");
                let record_id = owner_id
                    .parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("record owner_id `{owner_id}` is not numeric"))?;
                if let Some(record) = self
                    .records
                    .iter_mut()
                    .find(|record| record.id == record_id)
                    && matches!(event.value, SourceValue::Tag(ref key) if key == "Enter")
                {
                    record.edit_focus = false;
                }
                results.push(self.emit_frame_owned(self.record_change_paths(), metrics));
            } else if self.dynamic_text_event_matches(&event.path) {
                let owner_id = event
                    .owner_id
                    .as_deref()
                    .expect("dynamic edit_input event owner_id was validated");
                let record_id = owner_id
                    .parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("record owner_id `{owner_id}` is not numeric"))?;
                if let Some(record) = self
                    .records
                    .iter_mut()
                    .find(|record| record.id == record_id)
                    && event.path.ends_with(".event.blur")
                {
                    record.edit_focus = false;
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
                    && let Some(kinematics) = self.program.kinematics.as_ref()
                {
                    match kinematics.primary_control.axis {
                        ControlAxis::Horizontal => match key.as_str() {
                            "ArrowLeft" | "ArrowUp" => {
                                self.kinematics.control_x = (self.kinematics.control_x
                                    - kinematics.primary_control.step)
                                    .max(0);
                            }
                            "ArrowRight" | "ArrowDown" => {
                                self.kinematics.control_x = (self.kinematics.control_x
                                    + kinematics.primary_control.step)
                                    .min(100);
                            }
                            _ => {}
                        },
                        ControlAxis::Vertical => match key.as_str() {
                            "ArrowUp" | "ArrowLeft" => {
                                self.kinematics.control_y = (self.kinematics.control_y
                                    - kinematics.primary_control.step)
                                    .max(0);
                            }
                            "ArrowDown" | "ArrowRight" => {
                                self.kinematics.control_y = (self.kinematics.control_y
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
        self.clock_value = ticks as i64;
        self.emit_frame(&["clock", "clock_value"], TurnMetrics::default())
    }

    fn snapshot(&self) -> AppSnapshot {
        let mark = self.records.iter().filter(|record| record.mark).count() as i64;
        let mut values = BTreeMap::new();
        values.insert("scalar_value".to_string(), json!(self.scalar_value));
        for (path, value) in &self.generic_state {
            values.insert(path.clone(), json!(value));
        }
        if let Some(root) = self.record_root() {
            values.insert(
                format!("store.{root}_count"),
                json!(self.records.len() as i64),
            );
            values.insert(format!("store.marked_{root}_count"), json!(mark));
            values.insert(
                format!("store.unmarked_{root}_count"),
                json!(self.records.len() as i64 - mark),
            );
        }
        values.insert("clock_value".to_string(), json!(self.clock_value));
        if let Some(sequence) = &self.wiring.sequence {
            if let Some(entry_text) = &sequence.entry_text {
                values.insert(entry_text.clone(), json!(self.entry_text));
            }
            values.insert(
                format!("store.{}_titles", sequence.root),
                json!(
                    self.records
                        .iter()
                        .map(|record| record.content_text.clone())
                        .collect::<Vec<_>>()
                ),
            );
            values.insert(
                format!("store.{}_ids", sequence.root),
                json!(
                    self.records
                        .iter()
                        .map(|record| record.id)
                        .collect::<Vec<_>>()
                ),
            );
            values.insert(
                format!("store.visible_{}_ids", sequence.root),
                json!(
                    self.visible_records()
                        .map(|record| record.id)
                        .collect::<Vec<_>>()
                ),
            );
            values.insert(view_selector_state_key(), json!(self.view_selector));
            for record in &self.records {
                values.insert(
                    format!("store.{}[{}].content_text", sequence.root, record.id),
                    json!(record.content_text),
                );
                values.insert(
                    format!("store.{}[{}].mark", sequence.root, record.id),
                    json!(record.mark),
                );
                values.insert(
                    format!("store.{}[{}].edit_focus", sequence.root, record.id),
                    json!(record.edit_focus),
                );
            }
        }
        values.insert("kinematics.frame".to_string(), json!(self.frame_index));
        values.insert(
            "kinematics.control_y".to_string(),
            json!(self.kinematics.control_y),
        );
        values.insert(
            "kinematics.control_x".to_string(),
            json!(self.kinematics.control_x),
        );
        values.insert(
            "kinematics.tracked_control_y".to_string(),
            json!(self.kinematics.tracked_control_y),
        );
        values.insert(
            "kinematics.body_x".to_string(),
            json!(self.kinematics.body_x),
        );
        values.insert(
            "kinematics.body_y".to_string(),
            json!(self.kinematics.body_y),
        );
        values.insert(
            "kinematics.body_dx".to_string(),
            json!(self.kinematics.body_dx),
        );
        values.insert(
            "kinematics.body_dy".to_string(),
            json!(self.kinematics.body_dy),
        );
        values.insert(
            "kinematics.contact_field_rows".to_string(),
            json!(self.kinematics.contact_field_rows as i64),
        );
        values.insert(
            "kinematics.contact_field_cols".to_string(),
            json!(self.kinematics.contact_field_cols as i64),
        );
        values.insert(
            "kinematics.contact_field_live_count".to_string(),
            json!(
                self.kinematics
                    .contact_field
                    .iter()
                    .filter(|live| **live)
                    .count() as i64
            ),
        );
        values.insert(
            "kinematics.contact_value".to_string(),
            json!(self.kinematics.contact_value),
        );
        values.insert(
            "kinematics.resets_remaining".to_string(),
            json!(self.kinematics.resets_remaining),
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
