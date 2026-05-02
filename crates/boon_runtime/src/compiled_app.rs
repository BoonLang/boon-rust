use crate::{
    AppSnapshot, BoonApp, FakeClock, SourceBatch, SourceEmission, SourceInventory, SourceValue,
    StateDelta, TurnId, TurnMetrics, TurnResult,
};
use anyhow::{Result, bail};
use boon_compiler::{ControlAxis, ProgramSpec, SurfaceKind};
use boon_render_ir::{DrawCommand, FrameScene, HostPatch, NodeId, NodeKind};
use boon_shape::Shape;
use boon_source::{SourceEntry, SourceOwner};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ExampleApp {
    program: ProgramSpec,
    inventory: SourceInventory,
    wiring: RuntimeWiring,
    turn: u64,
    frame_text: String,
    counter: i64,
    interval_count: i64,
    clock: FakeClock,
    list_items: Vec<ListItem>,
    next_list_item_id: u64,
    input_text: String,
    source_state: BTreeMap<String, SourceValue>,
    filter: String,
    table: FormulaTableState,
    game_frame: u64,
    game: PlayfieldState,
}

#[derive(Clone, Debug, Default)]
struct RuntimeWiring {
    counter_event: Option<String>,
    collection: Option<CollectionBinding>,
    table: Option<TableBinding>,
    playfield_frame_event: Option<String>,
    playfield_control_event: Option<String>,
}

#[derive(Clone, Debug)]
struct CollectionBinding {
    family: String,
    root: String,
    input_text: Option<String>,
    input_key: Option<String>,
    input_focus: Option<String>,
    input_blur: Option<String>,
    input_change: Option<String>,
    toggle_all: Option<String>,
    clear_completed: Option<String>,
    filter_events: BTreeMap<String, String>,
    item_checkbox: Option<String>,
    item_remove: Option<String>,
    item_edit_text: Option<String>,
    item_edit_key: Option<String>,
    item_edit_blur: Option<String>,
    item_edit_change: Option<String>,
}

#[derive(Clone, Debug)]
struct TableBinding {
    family: String,
    root: String,
    display_double_click: Option<String>,
    editor_text: Option<String>,
    editor_key: Option<String>,
    viewport_key: Option<String>,
}

impl RuntimeWiring {
    fn from_program(program: &ProgramSpec, inventory: &SourceInventory) -> Self {
        let counter_event = program
            .scalar_counter
            .as_ref()
            .map(|counter| counter.event_path.clone());
        let collection = program
            .collection
            .as_ref()
            .and_then(|_| CollectionBinding::from_inventory(inventory));
        let table = program
            .table
            .as_ref()
            .and_then(|_| TableBinding::from_inventory(inventory));
        let playfield_frame_event = program
            .playfield
            .as_ref()
            .map(|playfield| playfield.frame_event_path.clone());
        let playfield_control_event = program
            .playfield
            .as_ref()
            .map(|playfield| playfield.control_event_path.clone());
        Self {
            counter_event,
            collection,
            table,
            playfield_frame_event,
            playfield_control_event,
        }
    }
}

impl CollectionBinding {
    fn from_inventory(inventory: &SourceInventory) -> Option<Self> {
        let family = first_dynamic_family(inventory, "Element/checkbox(element.event.click)")
            .or_else(|| first_dynamic_family(inventory, "Element/text_input(element.text)"))?;
        let root = dynamic_family_root(&family);
        let input_base = static_base_for_producer(inventory, "Element/text_input(element.text)");
        let item_edit_base =
            dynamic_base_for_producer(inventory, &family, "Element/text_input(element.text)");
        let mut filter_events = BTreeMap::new();
        for event in static_paths_for_producer(inventory, "Element/button(element.event.press)") {
            let Some(base) = event.strip_suffix(".event.press") else {
                continue;
            };
            let Some(name) = base.rsplit('.').next() else {
                continue;
            };
            if let Some(filter) = name.strip_prefix("filter_") {
                filter_events.insert(filter.to_string(), event);
            }
        }
        let clear_completed =
            static_paths_for_producer(inventory, "Element/button(element.event.press)")
                .into_iter()
                .find(|path| {
                    path.strip_suffix(".event.press")
                        .and_then(|base| base.rsplit('.').next())
                        .is_none_or(|name| !name.starts_with("filter_"))
                });
        Some(Self {
            family: family.clone(),
            root,
            input_text: input_base.as_ref().map(|base| format!("{base}.text")),
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
            toggle_all: static_paths_for_producer(
                inventory,
                "Element/checkbox(element.event.click)",
            )
            .into_iter()
            .next(),
            clear_completed,
            filter_events,
            item_checkbox: dynamic_path_for_producer(
                inventory,
                &family,
                "Element/checkbox(element.event.click)",
            ),
            item_remove: dynamic_path_for_producer(
                inventory,
                &family,
                "Element/button(element.event.press)",
            ),
            item_edit_text: item_edit_base.as_ref().map(|base| format!("{base}.text")),
            item_edit_key: item_edit_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.key_down.key"))),
            item_edit_blur: item_edit_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.blur"))),
            item_edit_change: item_edit_base
                .as_ref()
                .and_then(|base| existing_path(inventory, &format!("{base}.event.change"))),
        })
    }
}

impl TableBinding {
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
struct ListItem {
    id: u64,
    generation: u32,
    title: String,
    completed: bool,
    editing: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FormulaTableState {
    rows: usize,
    columns: usize,
    selected: (usize, usize),
    editing: Option<(usize, usize)>,
    text: Vec<String>,
    value: Vec<String>,
    deps: Vec<Vec<usize>>,
    rev_deps: Vec<Vec<usize>>,
}

impl FormulaTableState {
    fn new(rows: usize, columns: usize) -> Self {
        let len = rows * columns;
        Self {
            rows,
            columns,
            selected: (1, 1),
            editing: None,
            text: vec![String::new(); len],
            value: vec![String::new(); len],
            deps: vec![Vec::new(); len],
            rev_deps: vec![Vec::new(); len],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlayfieldState {
    ball_x: i64,
    ball_y: i64,
    ball_dx: i64,
    ball_dy: i64,
    control_x: i64,
    control_y: i64,
    peer_control_y: i64,
    bricks_rows: usize,
    bricks_cols: usize,
    bricks: Vec<bool>,
    score: i64,
    lives: i64,
}

impl PlayfieldState {
    fn from_spec(spec: Option<&boon_compiler::PlayfieldSpec>) -> Self {
        let Some(spec) = spec else {
            return Self::default();
        };
        let bricks_rows = spec.bricks.as_ref().map_or(0, |bricks| bricks.rows);
        let bricks_cols = spec.bricks.as_ref().map_or(0, |bricks| bricks.columns);
        Self {
            ball_x: spec.ball.x,
            ball_y: spec.ball.y,
            ball_dx: spec.ball.dx,
            ball_dy: spec.ball.dy,
            control_x: if matches!(spec.player.axis, ControlAxis::Horizontal) {
                spec.player.position
            } else {
                50
            },
            control_y: if matches!(spec.player.axis, ControlAxis::Vertical) {
                spec.player.position
            } else {
                50
            },
            peer_control_y: spec
                .opponent
                .as_ref()
                .map_or(50, |opponent| opponent.position),
            bricks_rows,
            bricks_cols,
            bricks: vec![true; bricks_rows * bricks_cols],
            score: 0,
            lives: 3,
        }
    }

    fn live_brick_indices(&self) -> String {
        self.bricks
            .iter()
            .enumerate()
            .filter_map(|(idx, live)| live.then_some(idx.to_string()))
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl Default for PlayfieldState {
    fn default() -> Self {
        Self {
            ball_x: 0,
            ball_y: 0,
            ball_dx: 0,
            ball_dy: 0,
            control_x: 50,
            control_y: 50,
            peer_control_y: 50,
            bricks_rows: 0,
            bricks_cols: 0,
            bricks: Vec::new(),
            score: 0,
            lives: 3,
        }
    }
}

impl ExampleApp {
    pub fn new(compiled: boon_compiler::CompiledModule) -> Self {
        let inventory = compiled.sources;
        let program = compiled.program;
        let wiring = RuntimeWiring::from_program(&program, &inventory);
        let initial_titles = program
            .collection
            .as_ref()
            .map(|list_item| list_item.initial_titles.clone())
            .unwrap_or_default();
        let table = program
            .table
            .as_ref()
            .map(|table| FormulaTableState::new(table.rows, table.columns))
            .unwrap_or_else(|| FormulaTableState::new(100, 26));
        let game = PlayfieldState::from_spec(program.playfield.as_ref());
        let mut app = Self {
            program,
            inventory,
            wiring,
            turn: 0,
            frame_text: String::new(),
            counter: 0,
            interval_count: 0,
            clock: FakeClock::default(),
            list_items: initial_titles
                .into_iter()
                .enumerate()
                .map(|(idx, title)| ListItem {
                    id: idx as u64 + 1,
                    generation: 0,
                    title,
                    completed: false,
                    editing: false,
                })
                .collect(),
            next_list_item_id: 1,
            input_text: String::new(),
            source_state: BTreeMap::new(),
            filter: "all".to_string(),
            table,
            game_frame: 0,
            game,
        };
        app.next_list_item_id = app.list_items.len() as u64 + 1;
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

    fn list_root(&self) -> Option<&str> {
        self.wiring
            .collection
            .as_ref()
            .map(|collection| collection.root.as_str())
    }

    fn list_state_prefix(&self) -> Option<String> {
        self.list_root().map(|root| format!("store.{root}"))
    }

    fn list_change_paths(&self) -> Vec<String> {
        self.list_state_prefix()
            .into_iter()
            .chain(
                self.wiring
                    .collection
                    .as_ref()
                    .and_then(|collection| collection.input_text.clone()),
            )
            .collect()
    }

    fn list_count_change_paths(&self) -> Vec<String> {
        let Some(root) = self.list_root() else {
            return Vec::new();
        };
        vec![
            format!("store.{root}_count"),
            format!("store.completed_{root}_count"),
            format!("store.active_{root}_count"),
        ]
    }

    fn list_input_change_paths(&self) -> Vec<String> {
        self.wiring
            .collection
            .as_ref()
            .and_then(|collection| collection.input_text.clone())
            .into_iter()
            .collect()
    }

    fn grid_change_paths(&self) -> Vec<String> {
        self.wiring
            .table
            .as_ref()
            .map(|table| vec![table.root.clone()])
            .unwrap_or_default()
    }

    fn grid_selection_change_paths(&self) -> Vec<String> {
        self.wiring
            .table
            .as_ref()
            .map(|table| vec![format!("{}.selected", table.root)])
            .unwrap_or_default()
    }

    fn list_static_text_event_matches(&self, path: &str) -> bool {
        self.wiring.collection.as_ref().is_some_and(|collection| {
            [
                &collection.input_focus,
                &collection.input_blur,
                &collection.input_change,
            ]
            .into_iter()
            .flatten()
            .any(|candidate| candidate == path)
        })
    }

    fn list_dynamic_text_event_matches(&self, path: &str) -> bool {
        self.wiring.collection.as_ref().is_some_and(|collection| {
            [&collection.item_edit_blur, &collection.item_edit_change]
                .into_iter()
                .flatten()
                .any(|candidate| candidate == path)
        })
    }

    fn filter_for_event_path(&self, path: &str) -> Option<String> {
        self.wiring.collection.as_ref().and_then(|collection| {
            collection
                .filter_events
                .iter()
                .find_map(|(filter, event_path)| (event_path == path).then(|| filter.clone()))
        })
    }

    fn render_text(&self) -> String {
        match self.program.scene {
            SurfaceKind::Collection => self.render_collection_text(),
            SurfaceKind::Table => self.render_table_text(),
            SurfaceKind::ActionValue => {
                let label = self
                    .program
                    .scalar_counter
                    .as_ref()
                    .map(|counter| counter.button_label.as_str())
                    .unwrap_or("Increment");
                format!(
                    "{}\nsurface: button-counter\n[ {label} ]\ncount: {}",
                    self.program.title, self.counter
                )
            }
            SurfaceKind::ClockValue => format!(
                "{}\nsurface: clock-counter\nfake_clock_ms: {}\nticks: {}",
                self.program.title, self.clock.millis, self.interval_count
            ),
            SurfaceKind::Playfield => self.render_playfield_text(),
            SurfaceKind::Blank => String::new(),
        }
    }

    fn render_scene(&self) -> FrameScene {
        let mut scene = FrameScene {
            title: self.program.title.clone(),
            commands: Vec::new(),
        };
        push_rect(&mut scene, 0, 0, 1000, 1000, [245, 245, 245, 255]);
        match self.program.scene {
            SurfaceKind::Collection => self.render_collection_scene(&mut scene),
            SurfaceKind::Table => self.render_table_scene(&mut scene),
            SurfaceKind::ActionValue => self.render_action_value_scene(&mut scene),
            SurfaceKind::ClockValue => self.render_clock_value_scene(&mut scene),
            SurfaceKind::Playfield => self.render_playfield_scene(&mut scene),
            SurfaceKind::Blank => {}
        }
        scene
    }

    fn render_action_value_scene(&self, scene: &mut FrameScene) {
        push_rect(scene, 0, 0, 1000, 1000, [238, 244, 247, 255]);
        push_text(scene, 84, 108, 3, &self.program.title, [25, 40, 52, 255]);
        push_rect(scene, 338, 388, 324, 92, [46, 125, 166, 255]);
        push_rect_outline(scene, 338, 388, 324, 92, [21, 91, 128, 255]);
        let label = self
            .program
            .scalar_counter
            .as_ref()
            .map(|counter| counter.button_label.as_str())
            .unwrap_or("Increment");
        push_text(scene, 424, 424, 2, label, [255, 255, 255, 255]);
        push_text(
            scene,
            424,
            548,
            3,
            &format!("count {}", self.counter),
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
            &format!("ticks {}", self.interval_count),
            [240, 250, 255, 255],
        );
        push_text(
            scene,
            184,
            456,
            2,
            &format!("fake clock {} ms", self.clock.millis),
            [166, 207, 224, 255],
        );
    }

    fn render_collection_scene(&self, scene: &mut FrameScene) {
        push_rect(scene, 0, 0, 1000, 1000, [245, 245, 245, 255]);
        push_text(scene, 340, 66, 4, "todos", [186, 137, 137, 255]);
        push_rect(scene, 206, 160, 588, 72, [255, 255, 255, 255]);
        push_rect_outline(scene, 206, 160, 588, 72, [225, 225, 225, 255]);
        push_rect_outline(scene, 226, 184, 28, 28, [198, 198, 198, 255]);
        push_text(scene, 234, 191, 1, "v", [116, 116, 116, 255]);
        push_text(
            scene,
            274,
            186,
            2,
            if self.input_text.is_empty() {
                "What needs to be done?"
            } else {
                &self.input_text
            },
            if self.input_text.is_empty() {
                [180, 180, 180, 255]
            } else {
                [54, 54, 54, 255]
            },
        );
        let visible_items: Vec<_> = self.visible_keyed_items().collect();
        let mut y = 234;
        for item in &visible_items {
            if y >= 1000 {
                break;
            }
            push_rect(scene, 206, y, 588, 62, [255, 255, 255, 255]);
            push_rect_outline(scene, 206, y, 588, 62, [232, 232, 232, 255]);
            push_rect_outline(scene, 226, y + 18, 24, 24, [126, 178, 164, 255]);
            if item.completed {
                push_text(scene, 231, y + 19, 1, "x", [68, 146, 126, 255]);
            }
            push_text(
                scene,
                270,
                y + 22,
                2,
                &item.title,
                if item.completed {
                    [160, 160, 160, 255]
                } else {
                    [60, 60, 60, 255]
                },
            );
            push_text(scene, 744, y + 22, 1, "x", [172, 84, 84, 255]);
            y += 62;
        }
        let completed = self.list_items.iter().filter(|item| item.completed).count();
        let active = self.list_items.len().saturating_sub(completed);
        y = 234 + visible_items.len() as u32 * 62;
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
            &format!("{active} items left"),
            [116, 116, 116, 255],
        );
        let filters = [
            ("all", 366, 64),
            ("active", 438, 84),
            ("completed", 536, 116),
        ];
        for (filter, x, outline_w) in filters {
            if self.filter == filter {
                push_rect_outline(scene, x - 10, y + 12, outline_w, 28, [218, 185, 185, 255]);
            }
            push_text(scene, x, y + 21, 1, filter, [116, 116, 116, 255]);
        }
        if completed > 0 {
            push_text(
                scene,
                680,
                y + 21,
                1,
                "Clear completed",
                [116, 116, 116, 255],
            );
        }
        push_text(
            scene,
            342,
            y + 82,
            1,
            "Double-click to edit an item",
            [150, 150, 150, 255],
        );
        push_text(
            scene,
            366,
            y + 116,
            1,
            "Created by Boon",
            [150, 150, 150, 255],
        );
        push_text(
            scene,
            338,
            y + 150,
            1,
            "Part of the classic app examples",
            [150, 150, 150, 255],
        );
        if self.program.physical_debug {
            push_text(
                scene,
                232,
                y + 184,
                1,
                "physical debug: depth bounds and source bindings stable",
                [115, 130, 145, 255],
            );
        }
    }

    fn render_table_scene(&self, scene: &mut FrameScene) {
        push_rect(scene, 0, 0, 1000, 1000, [248, 249, 250, 255]);
        let selected = format!(
            "{}{}",
            column_name(self.table.selected.1),
            self.table.selected.0
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
            self.grid_text(self.table.selected.0, self.table.selected.1),
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
                let selected_cell = self.table.selected == (row as usize, col as usize);
                push_rect(
                    scene,
                    x,
                    y,
                    col_w,
                    row_h,
                    if selected_cell {
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
                    if selected_cell {
                        [57, 132, 198, 255]
                    } else {
                        [214, 222, 228, 255]
                    },
                );
                let value = self.grid_value(row as usize, col as usize);
                if !value.is_empty() {
                    push_text(scene, x + 8, y + 14, 1, value, [40, 55, 68, 255]);
                }
            }
        }
    }

    fn render_playfield_scene(&self, scene: &mut FrameScene) {
        let Some(playfield) = self.program.playfield.as_ref() else {
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
                "frame {} score {} lives {}",
                self.game_frame, self.game.score, self.game.lives
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
            x0 + ((value.clamp(0, playfield.arena_width) as u32) * w
                / playfield.arena_width.max(1) as u32)
        };
        let sy = |value: i64| {
            y0 + ((value.clamp(0, playfield.arena_height) as u32) * h
                / playfield.arena_height.max(1) as u32)
        };
        let sw =
            |value: i64| ((value.max(1) as u32) * w / playfield.arena_width.max(1) as u32).max(1);
        let sh =
            |value: i64| ((value.max(1) as u32) * h / playfield.arena_height.max(1) as u32).max(1);

        if let Some(bricks) = &playfield.bricks {
            let brick_w = (playfield.arena_width
                - bricks.margin * 2
                - (bricks.columns.saturating_sub(1) as i64 * bricks.gap))
                / bricks.columns.max(1) as i64;
            for row in 0..bricks.rows {
                for col in 0..bricks.columns {
                    let idx = row * bricks.columns + col;
                    if self.game.bricks.get(idx).copied().unwrap_or(false) {
                        let bx = bricks.margin + col as i64 * (brick_w + bricks.gap);
                        let by = bricks.top + row as i64 * (bricks.height + bricks.gap);
                        let color = match row % 4 {
                            0 => [232, 92, 80, 255],
                            1 => [236, 168, 72, 255],
                            2 => [86, 176, 122, 255],
                            _ => [78, 146, 210, 255],
                        };
                        push_rect(scene, sx(bx), sy(by), sw(brick_w), sh(bricks.height), color);
                    }
                }
            }
        }

        let player_x = if playfield.player.axis == ControlAxis::Horizontal {
            paddle_left_from_position(
                self.game.control_x,
                playfield.arena_width,
                playfield.player.width,
            )
        } else {
            playfield.player.x
        };
        let player_y = if playfield.player.axis == ControlAxis::Vertical {
            paddle_top_from_position(
                self.game.control_y,
                playfield.arena_height,
                playfield.player.height,
            )
        } else {
            playfield.player.y
        };
        push_rect(
            scene,
            sx(player_x),
            sy(player_y),
            sw(playfield.player.width),
            sh(playfield.player.height),
            [85, 212, 230, 255],
        );
        if let Some(opponent) = &playfield.opponent {
            let opponent_y = paddle_top_from_position(
                self.game.peer_control_y,
                playfield.arena_height,
                opponent.height,
            );
            push_rect(
                scene,
                sx(opponent.x),
                sy(opponent_y),
                sw(opponent.width),
                sh(opponent.height),
                [240, 244, 247, 255],
            );
        }
        push_rect(
            scene,
            sx(self.game.ball_x),
            sy(self.game.ball_y),
            sw(playfield.ball.size),
            sh(playfield.ball.size),
            [250, 250, 250, 255],
        );
    }

    fn render_collection_text(&self) -> String {
        let completed = self
            .list_items
            .iter()
            .filter(|list_item| list_item.completed)
            .count();
        let active = self.list_items.len().saturating_sub(completed);
        let mut lines = vec![
            self.program.title.clone(),
            "surface: collection".to_string(),
            "What needs to be done?".to_string(),
            format!("input: {}", self.input_text),
        ];
        for list_item in self.visible_keyed_items() {
            lines.push(format!(
                "{} [{}] {}",
                list_item.id,
                if list_item.completed { "x" } else { " " },
                list_item.title
            ));
        }
        lines.push(format!("{active} items left"));
        lines.push(format!("filter: {}", self.filter));
        if self.program.physical_debug {
            lines.push("physical/debug: depth bounds source-bindings stable".to_string());
        }
        lines.join("\n")
    }

    fn render_playfield_text(&self) -> String {
        let frame_source = self
            .wiring
            .playfield_frame_event
            .as_deref()
            .unwrap_or("frame source");
        format!(
            "{}\nsurface: playfield\nplayfield_mode: {}\nframe: {}\ncontrol_y: {}\ncontrol_x: {}\npeer_control_y: {}\nball_x: {}\nball_y: {}\nball_dx: {}\nball_dy: {}\nbricks_rows: {}\nbricks_cols: {}\nobstacles_active: {}\nscore: {}\nlives: {}\ndeterministic input source: {}",
            self.program.title,
            if self
                .program
                .playfield
                .as_ref()
                .and_then(|playfield| playfield.bricks.as_ref())
                .is_some()
            {
                "obstacle-field"
            } else {
                "dual-walls"
            },
            self.game_frame,
            self.game.control_y,
            self.game.control_x,
            self.game.peer_control_y,
            self.game.ball_x,
            self.game.ball_y,
            self.game.ball_dx,
            self.game.ball_dy,
            self.game.bricks_rows,
            self.game.bricks_cols,
            self.game.live_brick_indices(),
            self.game.score,
            self.game.lives,
            frame_source
        )
    }

    fn advance_playfield_step(&mut self) {
        self.game_frame += 1;
        if self
            .program
            .playfield
            .as_ref()
            .and_then(|playfield| playfield.bricks.as_ref())
            .is_some()
        {
            self.advance_obstacle_field_step();
        } else {
            self.advance_dual_wall_step();
        }
    }

    fn advance_dual_wall_step(&mut self) {
        let Some(playfield) = self.program.playfield.as_ref() else {
            return;
        };
        let Some(opponent) = playfield.opponent.as_ref() else {
            return;
        };
        let arena_w = playfield.arena_width;
        let arena_h = playfield.arena_height;
        let ball = playfield.ball.size;
        let paddle_w = playfield.player.width;
        let paddle_h = playfield.player.height;
        let left_x = playfield.player.x;
        let right_x = opponent.x;

        self.game.ball_x += self.game.ball_dx;
        self.game.ball_y += self.game.ball_dy;
        if self.game.ball_y <= 0 {
            self.game.ball_y = 0;
            self.game.ball_dy = self.game.ball_dy.abs();
        } else if self.game.ball_y + ball >= arena_h {
            self.game.ball_y = arena_h - ball;
            self.game.ball_dy = -self.game.ball_dy.abs();
        }

        self.game.peer_control_y = position_from_paddle_top(
            self.game.ball_y + ball / 2 - paddle_h / 2,
            arena_h,
            paddle_h,
        );
        let left_y = paddle_top_from_position(self.game.control_y, arena_h, paddle_h);
        let right_y = paddle_top_from_position(self.game.peer_control_y, arena_h, paddle_h);

        if self.game.ball_dx < 0
            && self.game.ball_x <= left_x + paddle_w
            && self.game.ball_x + ball >= left_x
            && ranges_overlap(
                self.game.ball_y,
                self.game.ball_y + ball,
                left_y,
                left_y + paddle_h,
            )
        {
            self.game.ball_x = left_x + paddle_w;
            self.game.ball_dx = self.game.ball_dx.abs();
            self.game.ball_dy = (self.game.ball_dy
                + ((self.game.ball_y + ball / 2) - (left_y + paddle_h / 2)) / 18)
                .clamp(-18, 18);
            self.game.score += 1;
        }
        if self.game.ball_dx > 0
            && self.game.ball_x + ball >= right_x
            && self.game.ball_x <= right_x + paddle_w
            && ranges_overlap(
                self.game.ball_y,
                self.game.ball_y + ball,
                right_y,
                right_y + paddle_h,
            )
        {
            self.game.ball_x = right_x - ball;
            self.game.ball_dx = -self.game.ball_dx.abs();
            self.game.ball_dy = (self.game.ball_dy
                + ((self.game.ball_y + ball / 2) - (right_y + paddle_h / 2)) / 18)
                .clamp(-18, 18);
            self.game.score += 1;
        }
        if self.game.ball_x < -ball || self.game.ball_x > arena_w + ball {
            self.game.ball_x = arena_w / 2;
            self.game.ball_y = arena_h / 2;
            self.game.ball_dx = if self.game.ball_dx < 0 { 12 } else { -12 };
            self.game.ball_dy = 8;
            self.game.lives = (self.game.lives - 1).max(0);
        }
    }

    fn advance_obstacle_field_step(&mut self) {
        let Some(playfield) = self.program.playfield.as_ref() else {
            return;
        };
        let Some(bricks) = playfield.bricks.as_ref() else {
            return;
        };
        let arena_w = playfield.arena_width;
        let arena_h = playfield.arena_height;
        let ball = playfield.ball.size;
        let paddle_w = playfield.player.width;
        let paddle_h = playfield.player.height;
        let control_y = playfield.player.y;

        self.game.ball_x += self.game.ball_dx;
        self.game.ball_y += self.game.ball_dy;
        if self.game.ball_x <= 0 {
            self.game.ball_x = 0;
            self.game.ball_dx = self.game.ball_dx.abs();
        } else if self.game.ball_x + ball >= arena_w {
            self.game.ball_x = arena_w - ball;
            self.game.ball_dx = -self.game.ball_dx.abs();
        }
        if self.game.ball_y <= 0 {
            self.game.ball_y = 0;
            self.game.ball_dy = self.game.ball_dy.abs();
        }

        if self.game.ball_dy < 0 {
            let margin = bricks.margin;
            let gap = bricks.gap;
            let brick_h = bricks.height;
            let rows = self.game.bricks_rows as i64;
            let cols = self.game.bricks_cols as i64;
            let brick_w = if cols > 0 {
                (arena_w - margin * 2 - gap * (cols - 1)) / cols
            } else {
                0
            };
            'brick_scan: for row in 0..rows {
                for col in 0..cols {
                    let idx = (row * cols + col) as usize;
                    if !self.game.bricks.get(idx).copied().unwrap_or(false) {
                        continue;
                    }
                    let bx = margin + col * (brick_w + gap);
                    let by = bricks.top + row * (brick_h + gap);
                    if rects_overlap(
                        self.game.ball_x,
                        self.game.ball_y,
                        ball,
                        ball,
                        bx,
                        by,
                        brick_w,
                        brick_h,
                    ) {
                        self.game.bricks[idx] = false;
                        self.game.ball_dy = self.game.ball_dy.abs();
                        self.game.score += bricks.score_per_hit;
                        break 'brick_scan;
                    }
                }
            }
        }

        let control_x = paddle_left_from_position(self.game.control_x, arena_w, paddle_w);
        if self.game.ball_dy > 0
            && rects_overlap(
                self.game.ball_x,
                self.game.ball_y,
                ball,
                ball,
                control_x,
                control_y,
                paddle_w,
                paddle_h,
            )
        {
            self.game.ball_y = control_y - ball;
            self.game.ball_dy = -self.game.ball_dy.abs();
            self.game.ball_dx = (self.game.ball_dx
                + ((self.game.ball_x + ball / 2) - (control_x + paddle_w / 2)) / 18)
                .clamp(-18, 18);
        }
        if self.game.ball_y > arena_h {
            self.game.ball_x = control_x + paddle_w / 2 - ball / 2;
            self.game.ball_y = control_y - ball - 2;
            self.game.ball_dx = playfield.ball.dx;
            self.game.ball_dy = playfield.ball.dy;
            self.game.lives = (self.game.lives - 1).max(0);
        }
        if self.game.bricks.iter().all(|live| !*live) {
            self.game.bricks.fill(true);
        }
    }

    fn visible_keyed_items(&self) -> impl Iterator<Item = &ListItem> {
        self.list_items
            .iter()
            .filter(|list_item| match self.filter.as_str() {
                "active" => !list_item.completed,
                "completed" => list_item.completed,
                _ => true,
            })
    }

    fn render_table_text(&self) -> String {
        let mut lines = vec![
            self.program.title.clone(),
            "surface: table".to_string(),
            format!(
                "selected: {}{}",
                column_name(self.table.selected.1),
                self.table.selected.0
            ),
            format!(
                "formula: {}",
                self.grid_text(self.table.selected.0, self.table.selected.1)
            ),
            format!(
                "value: {}",
                self.grid_value(self.table.selected.0, self.table.selected.1)
            ),
            "columns: A B C D E F ... Z".to_string(),
        ];
        for row in 1..=self.table.rows.min(5) {
            lines.push(format!(
                "row {row}: A={} | B={} | C={}",
                self.grid_value(row, 1.min(self.table.columns)),
                self.grid_value(row, 2.min(self.table.columns)),
                self.grid_value(row, 3.min(self.table.columns))
            ));
        }
        lines.push(format!(
            "row {} and column {} reachable",
            self.table.rows,
            column_name(self.table.columns)
        ));
        lines.join("\n")
    }

    fn grid_value(&self, row: usize, col: usize) -> &str {
        &self.table.value[self.grid_idx(row, col)]
    }

    fn grid_text(&self, row: usize, col: usize) -> &str {
        &self.table.text[self.grid_idx(row, col)]
    }

    fn grid_idx(&self, row: usize, col: usize) -> usize {
        (row - 1) * self.table.columns + (col - 1)
    }

    fn set_grid_text(&mut self, row: usize, col: usize, text: String) {
        let idx = self.grid_idx(row, col);
        for dep in self.table.deps[idx].drain(..) {
            self.table.rev_deps[dep].retain(|dependent| *dependent != idx);
        }
        let deps = self.collect_formula_refs(&text);
        for dep in &deps {
            if !self.table.rev_deps[*dep].contains(&idx) {
                self.table.rev_deps[*dep].push(idx);
            }
        }
        self.table.deps[idx] = deps;
        self.table.text[idx] = text;
        self.recalc_dirty_grid(idx);
    }

    fn recalc_dirty_grid(&mut self, changed: usize) {
        let mut dirty = BTreeSet::new();
        self.collect_grid_dependents(changed, &mut dirty);
        let mut memo = BTreeMap::new();
        for idx in dirty {
            let value = self.evaluate_cell(idx, &mut BTreeSet::new(), &mut memo);
            self.table.value[idx] = value;
        }
    }

    fn collect_grid_dependents(&self, idx: usize, dirty: &mut BTreeSet<usize>) {
        if dirty.insert(idx) {
            for dependent in &self.table.rev_deps[idx] {
                self.collect_grid_dependents(*dependent, dirty);
            }
        }
    }

    fn evaluate_cell(
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
        let text = &self.table.text[idx];
        let value = if let Some(formula) = text.strip_prefix('=') {
            self.resolve_formula_text(formula, visiting, memo)
        } else {
            text.clone()
        };
        visiting.remove(&idx);
        memo.insert(idx, value.clone());
        value
    }

    fn resolve_formula_text(
        &self,
        formula: &str,
        visiting: &mut BTreeSet<usize>,
        memo: &mut BTreeMap<usize, String>,
    ) -> String {
        if self.cell_formula_enabled("add")
            && let Some(args) = formula
                .strip_prefix("add(")
                .and_then(|rest| rest.strip_suffix(')'))
        {
            let parts = args.split(',').map(str::trim).collect::<Vec<_>>();
            if parts.len() != 2 {
                return "#ERR".to_string();
            }
            let Some(left) = self.decode_table_ref(parts[0]) else {
                return "#ERR".to_string();
            };
            let Some(right) = self.decode_table_ref(parts[1]) else {
                return "#ERR".to_string();
            };
            let Some(left) = self.evaluate_grid_number(left, visiting, memo) else {
                return "#CYCLE".to_string();
            };
            let Some(right) = self.evaluate_grid_number(right, visiting, memo) else {
                return "#CYCLE".to_string();
            };
            return (left + right).to_string();
        }
        if self.cell_formula_enabled("sum")
            && let Some(args) = formula
                .strip_prefix("sum(")
                .and_then(|rest| rest.strip_suffix(')'))
        {
            let Some((start, end)) = self.parse_cell_range(args.trim()) else {
                return "#ERR".to_string();
            };
            let mut sum = 0;
            for row in start.0.min(end.0)..=start.0.max(end.0) {
                for col in start.1.min(end.1)..=start.1.max(end.1) {
                    let Some(value) =
                        self.evaluate_grid_number(self.grid_idx(row, col), visiting, memo)
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

    fn cell_formula_enabled(&self, name: &str) -> bool {
        self.program.table.as_ref().is_some_and(|table| {
            table
                .formula_functions
                .iter()
                .any(|function| function == name)
        })
    }

    fn collect_formula_refs(&self, text: &str) -> Vec<usize> {
        let Some(formula) = text.strip_prefix('=') else {
            return Vec::new();
        };
        if self.cell_formula_enabled("add")
            && let Some(args) = formula
                .strip_prefix("add(")
                .and_then(|rest| rest.strip_suffix(')'))
        {
            return args
                .split(',')
                .filter_map(|arg| self.decode_table_ref(arg.trim()))
                .collect();
        }
        if self.cell_formula_enabled("sum")
            && let Some(args) = formula
                .strip_prefix("sum(")
                .and_then(|rest| rest.strip_suffix(')'))
            && let Some((start, end)) = self.parse_cell_range(args.trim())
        {
            let mut deps = Vec::new();
            for row in start.0.min(end.0)..=start.0.max(end.0) {
                for col in start.1.min(end.1)..=start.1.max(end.1) {
                    deps.push(self.grid_idx(row, col));
                }
            }
            return deps;
        }
        Vec::new()
    }

    fn parse_cell_range(&self, text: &str) -> Option<((usize, usize), (usize, usize))> {
        let (start, end) = text.split_once(':')?;
        Some((
            self.decode_table_ref_tuple(start)?,
            self.decode_table_ref_tuple(end)?,
        ))
    }

    fn decode_table_ref(&self, text: &str) -> Option<usize> {
        let (row, col) = self.decode_table_ref_tuple(text)?;
        Some(self.grid_idx(row, col))
    }

    fn decode_table_ref_tuple(&self, text: &str) -> Option<(usize, usize)> {
        let mut chars = text.chars();
        let col = chars.next()?.to_ascii_uppercase();
        if !col.is_ascii_uppercase() {
            return None;
        }
        let row = chars.as_str().parse::<usize>().ok()?;
        let col = (col as u8).checked_sub(b'A')? as usize + 1;
        if row == 0 || row > self.table.rows || col == 0 || col > self.table.columns {
            return None;
        }
        Some((row, col))
    }

    fn parse_grid_owner(&self, owner_id: &str) -> Result<(usize, usize)> {
        self.decode_table_ref_tuple(owner_id).ok_or_else(|| {
            anyhow::anyhow!("grid_cell owner_id `{owner_id}` is outside compiled table")
        })
    }

    fn evaluate_grid_number(
        &self,
        idx: usize,
        visiting: &mut BTreeSet<usize>,
        memo: &mut BTreeMap<usize, String>,
    ) -> Option<i64> {
        let value = self.evaluate_cell(idx, visiting, memo);
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
        if let Some(collection) = &self.wiring.collection
            && path.starts_with(&collection.family)
        {
            let list_item_id = owner_id
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("list_item owner_id `{owner_id}` is not numeric"))?;
            return self
                .list_items
                .iter()
                .find(|list_item| list_item.id == list_item_id)
                .map(|list_item| list_item.generation)
                .ok_or_else(|| {
                    anyhow::anyhow!("dynamic list_item owner `{owner_id}` is not live")
                });
        }
        if let Some(table) = &self.wiring.table
            && path.starts_with(&table.family)
        {
            self.parse_grid_owner(owner_id)?;
            return Ok(0);
        }
        bail!("dynamic SOURCE `{path}` has no owner generation table")
    }
}

impl BoonApp for ExampleApp {
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
                .collection
                .as_ref()
                .and_then(|collection| collection.input_text.as_ref())
                .is_some_and(|path| update.path == *path)
            {
                if let SourceValue::Text(value) = update.value {
                    self.input_text = value;
                }
            } else if self
                .wiring
                .collection
                .as_ref()
                .and_then(|collection| collection.item_edit_text.as_ref())
                .is_some_and(|path| update.path == *path)
                && let SourceValue::Text(value) = update.value
            {
                let owner_id = update
                    .owner_id
                    .as_deref()
                    .expect("dynamic edit text owner_id was validated");
                let list_item_id = owner_id.parse::<u64>().map_err(|_| {
                    anyhow::anyhow!("list_item owner_id `{owner_id}` is not numeric")
                })?;
                let list_item = self
                    .list_items
                    .iter_mut()
                    .find(|list_item| list_item.id == list_item_id)
                    .ok_or_else(|| anyhow::anyhow!("list_item owner `{owner_id}` is not live"))?;
                list_item.title = value;
                list_item.editing = true;
            } else if self
                .wiring
                .table
                .as_ref()
                .and_then(|table| table.editor_text.as_ref())
                .is_some_and(|path| update.path == *path)
                && let SourceValue::Text(value) = update.value
            {
                let (row, col) = update
                    .owner_id
                    .as_deref()
                    .map(|owner_id| self.parse_grid_owner(owner_id))
                    .transpose()?
                    .unwrap_or(self.table.selected);
                self.set_grid_text(row, col, value);
            }
        }

        let mut results = Vec::new();
        for event in batch.events {
            let mut metrics = TurnMetrics {
                events_processed: 1,
                ..TurnMetrics::default()
            };
            if self
                .wiring
                .counter_event
                .as_ref()
                .is_some_and(|path| event.path == *path)
            {
                self.counter += self
                    .program
                    .scalar_counter
                    .as_ref()
                    .map(|counter| counter.step)
                    .unwrap_or(0);
                results.push(self.emit_frame(&["counter"], metrics));
            } else if self
                .wiring
                .collection
                .as_ref()
                .and_then(|collection| collection.input_key.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                if matches!(event.value, SourceValue::Tag(ref key) if key == "Enter") {
                    let trimmed = self.input_text.trim().to_string();
                    if self
                        .program
                        .collection
                        .as_ref()
                        .is_some_and(|collection| collection.append_from_text_input)
                        && !trimmed.is_empty()
                    {
                        self.list_items.push(ListItem {
                            id: self.next_list_item_id,
                            generation: 0,
                            title: trimmed,
                            completed: false,
                            editing: false,
                        });
                        self.next_list_item_id += 1;
                    }
                    self.input_text.clear();
                }
                results.push(self.emit_frame_owned(self.list_change_paths(), metrics));
            } else if self.list_static_text_event_matches(&event.path) {
                results.push(self.emit_frame_owned(self.list_input_change_paths(), metrics));
            } else if self
                .wiring
                .collection
                .as_ref()
                .and_then(|collection| collection.toggle_all.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                if self
                    .program
                    .collection
                    .as_ref()
                    .is_some_and(|collection| collection.toggle_all_from_checkbox)
                {
                    let all_completed = self.list_items.iter().all(|list_item| list_item.completed);
                    for list_item in &mut self.list_items {
                        list_item.completed = !all_completed;
                    }
                }
                metrics.list_rows_touched = self.list_items.len();
                results.push(self.emit_frame_owned(self.list_count_change_paths(), metrics));
            } else if self
                .wiring
                .collection
                .as_ref()
                .and_then(|collection| collection.item_checkbox.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                let owner_id = event
                    .owner_id
                    .as_deref()
                    .expect("dynamic event owner_id was validated");
                let list_item_id = owner_id.parse::<u64>().map_err(|_| {
                    anyhow::anyhow!("list_item owner_id `{owner_id}` is not numeric")
                })?;
                let list_item = self
                    .list_items
                    .iter_mut()
                    .find(|list_item| list_item.id == list_item_id)
                    .ok_or_else(|| anyhow::anyhow!("list_item owner `{owner_id}` is not live"))?;
                if self
                    .program
                    .collection
                    .as_ref()
                    .is_some_and(|collection| collection.item_checkbox_toggle)
                {
                    list_item.completed = !list_item.completed;
                }
                metrics.list_rows_touched = 1;
                results.push(self.emit_frame_owned(self.list_count_change_paths(), metrics));
            } else if self
                .wiring
                .collection
                .as_ref()
                .and_then(|collection| collection.item_remove.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                let owner_id = event
                    .owner_id
                    .as_deref()
                    .expect("dynamic event owner_id was validated");
                let list_item_id = owner_id.parse::<u64>().map_err(|_| {
                    anyhow::anyhow!("list_item owner_id `{owner_id}` is not numeric")
                })?;
                if self
                    .program
                    .collection
                    .as_ref()
                    .is_some_and(|collection| collection.item_remove_button)
                {
                    self.list_items
                        .retain(|list_item| list_item.id != list_item_id);
                }
                results.push(self.emit_frame_owned(self.list_change_paths(), metrics));
            } else if self
                .wiring
                .collection
                .as_ref()
                .and_then(|collection| collection.clear_completed.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                if self
                    .program
                    .collection
                    .as_ref()
                    .is_some_and(|collection| collection.clear_completed_from_button)
                {
                    self.list_items.retain(|list_item| !list_item.completed);
                }
                results.push(self.emit_frame_owned(self.list_change_paths(), metrics));
            } else if let Some(filter) = self.filter_for_event_path(&event.path) {
                self.filter = filter;
                results.push(self.emit_frame(&["store.selected_filter"], metrics));
            } else if self
                .wiring
                .collection
                .as_ref()
                .and_then(|collection| collection.item_edit_key.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                let owner_id = event
                    .owner_id
                    .as_deref()
                    .expect("dynamic edit_input key owner_id was validated");
                let list_item_id = owner_id.parse::<u64>().map_err(|_| {
                    anyhow::anyhow!("list_item owner_id `{owner_id}` is not numeric")
                })?;
                if let Some(list_item) = self
                    .list_items
                    .iter_mut()
                    .find(|list_item| list_item.id == list_item_id)
                    && matches!(event.value, SourceValue::Tag(ref key) if key == "Enter")
                {
                    list_item.editing = false;
                }
                results.push(self.emit_frame_owned(self.list_change_paths(), metrics));
            } else if self.list_dynamic_text_event_matches(&event.path) {
                let owner_id = event
                    .owner_id
                    .as_deref()
                    .expect("dynamic edit_input event owner_id was validated");
                let list_item_id = owner_id.parse::<u64>().map_err(|_| {
                    anyhow::anyhow!("list_item owner_id `{owner_id}` is not numeric")
                })?;
                if let Some(list_item) = self
                    .list_items
                    .iter_mut()
                    .find(|list_item| list_item.id == list_item_id)
                    && event.path.ends_with(".event.blur")
                {
                    list_item.editing = false;
                }
                results.push(self.emit_frame_owned(self.list_change_paths(), metrics));
            } else if self
                .wiring
                .table
                .as_ref()
                .and_then(|table| table.display_double_click.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                let (row, col) = event
                    .owner_id
                    .as_deref()
                    .map(|owner_id| self.parse_grid_owner(owner_id))
                    .transpose()?
                    .unwrap_or(self.table.selected);
                self.table.selected = (row, col);
                self.table.editing = Some((row, col));
                results.push(self.emit_frame_owned(self.grid_change_paths(), metrics));
            } else if self
                .wiring
                .table
                .as_ref()
                .and_then(|table| table.editor_key.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                if matches!(event.value, SourceValue::Tag(ref key) if key == "Enter") {
                    self.table.editing = None;
                }
                results.push(self.emit_frame_owned(self.grid_change_paths(), metrics));
            } else if self
                .wiring
                .table
                .as_ref()
                .and_then(|table| table.viewport_key.as_ref())
                .is_some_and(|path| event.path == *path)
            {
                if let SourceValue::Tag(key) = &event.value {
                    match key.as_str() {
                        "ArrowUp" => {
                            self.table.selected.0 = self.table.selected.0.saturating_sub(1).max(1);
                        }
                        "ArrowDown" => {
                            self.table.selected.0 =
                                (self.table.selected.0 + 1).min(self.table.rows);
                        }
                        "ArrowLeft" => {
                            self.table.selected.1 = self.table.selected.1.saturating_sub(1).max(1);
                        }
                        "ArrowRight" => {
                            self.table.selected.1 =
                                (self.table.selected.1 + 1).min(self.table.columns);
                        }
                        _ => {}
                    }
                }
                results.push(self.emit_frame_owned(self.grid_selection_change_paths(), metrics));
            } else if self
                .wiring
                .playfield_control_event
                .as_ref()
                .is_some_and(|path| event.path == *path)
            {
                if let SourceValue::Tag(key) = &event.value
                    && let Some(playfield) = self.program.playfield.as_ref()
                {
                    match playfield.player.axis {
                        ControlAxis::Horizontal => match key.as_str() {
                            "ArrowLeft" | "ArrowUp" => {
                                self.game.control_x =
                                    (self.game.control_x - playfield.player.step).max(0);
                            }
                            "ArrowRight" | "ArrowDown" => {
                                self.game.control_x =
                                    (self.game.control_x + playfield.player.step).min(100);
                            }
                            _ => {}
                        },
                        ControlAxis::Vertical => match key.as_str() {
                            "ArrowUp" | "ArrowLeft" => {
                                self.game.control_y =
                                    (self.game.control_y - playfield.player.step).max(0);
                            }
                            "ArrowDown" | "ArrowRight" => {
                                self.game.control_y =
                                    (self.game.control_y + playfield.player.step).min(100);
                            }
                            _ => {}
                        },
                    }
                }
                results.push(self.emit_frame(&["game.control"], metrics));
            } else if self
                .wiring
                .playfield_frame_event
                .as_ref()
                .is_some_and(|path| event.path == *path)
            {
                self.advance_playfield_step();
                results.push(self.emit_frame(&["frame", "game.ball"], metrics));
            }
        }
        if results.is_empty() && !changed_paths.is_empty() {
            let changed = changed_paths.iter().map(String::as_str).collect::<Vec<_>>();
            results.push(self.emit_frame(&changed, TurnMetrics::default()));
        }
        Ok(results)
    }

    fn advance_fake_time(&mut self, delta: Duration) -> TurnResult {
        self.clock.advance(delta);
        let ticks = self.clock.millis / 1000;
        self.interval_count = ticks as i64;
        self.emit_frame(&["clock", "interval_count"], TurnMetrics::default())
    }

    fn snapshot(&self) -> AppSnapshot {
        let completed = self
            .list_items
            .iter()
            .filter(|list_item| list_item.completed)
            .count() as i64;
        let mut values = BTreeMap::new();
        values.insert("counter".to_string(), json!(self.counter));
        if let Some(root) = self.list_root() {
            values.insert(
                format!("store.{root}_count"),
                json!(self.list_items.len() as i64),
            );
            values.insert(format!("store.completed_{root}_count"), json!(completed));
            values.insert(
                format!("store.active_{root}_count"),
                json!(self.list_items.len() as i64 - completed),
            );
        }
        values.insert("interval_count".to_string(), json!(self.interval_count));
        if let Some(collection) = &self.wiring.collection {
            if let Some(input_text) = &collection.input_text {
                values.insert(input_text.clone(), json!(self.input_text));
            }
            values.insert(
                format!("store.{}_titles", collection.root),
                json!(
                    self.list_items
                        .iter()
                        .map(|list_item| list_item.title.clone())
                        .collect::<Vec<_>>()
                ),
            );
            values.insert(
                format!("store.{}_ids", collection.root),
                json!(
                    self.list_items
                        .iter()
                        .map(|list_item| list_item.id)
                        .collect::<Vec<_>>()
                ),
            );
            values.insert(
                format!("store.visible_{}_ids", collection.root),
                json!(
                    self.visible_keyed_items()
                        .map(|list_item| list_item.id)
                        .collect::<Vec<_>>()
                ),
            );
            values.insert("store.selected_filter".to_string(), json!(self.filter));
            for list_item in &self.list_items {
                values.insert(
                    format!("store.{}[{}].title", collection.root, list_item.id),
                    json!(list_item.title),
                );
                values.insert(
                    format!("store.{}[{}].completed", collection.root, list_item.id),
                    json!(list_item.completed),
                );
                values.insert(
                    format!("store.{}[{}].editing", collection.root, list_item.id),
                    json!(list_item.editing),
                );
            }
        }
        values.insert("game.frame".to_string(), json!(self.game_frame));
        values.insert("game.control_y".to_string(), json!(self.game.control_y));
        values.insert("game.control_x".to_string(), json!(self.game.control_x));
        values.insert(
            "game.peer_control_y".to_string(),
            json!(self.game.peer_control_y),
        );
        values.insert("game.ball_x".to_string(), json!(self.game.ball_x));
        values.insert("game.ball_y".to_string(), json!(self.game.ball_y));
        values.insert("game.ball_dx".to_string(), json!(self.game.ball_dx));
        values.insert("game.ball_dy".to_string(), json!(self.game.ball_dy));
        values.insert(
            "game.bricks_rows".to_string(),
            json!(self.game.bricks_rows as i64),
        );
        values.insert(
            "game.bricks_cols".to_string(),
            json!(self.game.bricks_cols as i64),
        );
        values.insert(
            "game.obstacles_live_count".to_string(),
            json!(self.game.bricks.iter().filter(|live| **live).count() as i64),
        );
        values.insert("game.score".to_string(), json!(self.game.score));
        values.insert("game.lives".to_string(), json!(self.game.lives));
        let grid_root = self
            .wiring
            .table
            .as_ref()
            .map(|table| table.root.as_str())
            .unwrap_or("table");
        values.insert(format!("{grid_root}.A1"), json!(self.grid_value(1, 1)));
        values.insert(format!("{grid_root}.A2"), json!(self.grid_value(2, 1)));
        values.insert(format!("{grid_root}.A3"), json!(self.grid_value(3, 1)));
        values.insert(format!("{grid_root}.B1"), json!(self.grid_value(1, 2)));
        values.insert(format!("{grid_root}.B2"), json!(self.grid_value(2, 2)));
        for (row, col, name) in [
            (1, 1, "A1"),
            (2, 1, "A2"),
            (3, 1, "A3"),
            (1, 2, "B1"),
            (2, 2, "B2"),
        ] {
            values.insert(
                format!("{grid_root}.{name}.formula"),
                json!(self.grid_text(row, col)),
            );
        }
        values.insert(
            format!("{grid_root}.selected_formula"),
            json!(self.grid_text(self.table.selected.0, self.table.selected.1)),
        );
        values.insert(
            format!("{grid_root}.selected_value"),
            json!(self.grid_value(self.table.selected.0, self.table.selected.1)),
        );
        values.insert(
            format!("{grid_root}.selected"),
            json!(format!(
                "{}{}",
                column_name(self.table.selected.1),
                self.table.selected.0
            )),
        );
        values.insert(
            format!("{grid_root}.editing"),
            json!(
                self.table
                    .editing
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

fn paddle_top_from_position(position: i64, arena_h: i64, paddle_h: i64) -> i64 {
    ((arena_h - paddle_h).max(0) * position.clamp(0, 100) / 100).clamp(0, arena_h - paddle_h)
}

fn paddle_left_from_position(position: i64, arena_w: i64, paddle_w: i64) -> i64 {
    ((arena_w - paddle_w).max(0) * position.clamp(0, 100) / 100).clamp(0, arena_w - paddle_w)
}

fn position_from_paddle_top(top: i64, arena_h: i64, paddle_h: i64) -> i64 {
    let span = (arena_h - paddle_h).max(1);
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
