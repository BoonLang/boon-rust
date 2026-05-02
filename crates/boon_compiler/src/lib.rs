use anyhow::{Result, bail};
use boon_hir::{HirModule, lower};
use boon_host_schema::{HostContract, element_contracts};
use boon_shape::Shape;
use boon_source::{SourceEntry, SourceInventory, SourceOwner};
use boon_syntax::parse_module;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompiledModule {
    pub name: String,
    pub hir: HirModule,
    pub sources: SourceInventory,
    pub program: ProgramSpec,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProgramSpec {
    pub title: String,
    pub scene: SurfaceKind,
    pub scalar_counter: Option<AccumulatorSpec>,
    pub timer_counter: Option<ClockAccumulatorSpec>,
    pub collection: Option<CollectionSpec>,
    pub table: Option<TableSpec>,
    pub playfield: Option<PlayfieldSpec>,
    pub physical_debug: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    #[default]
    Blank,
    ActionValue,
    ClockValue,
    Collection,
    Table,
    Playfield,
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
pub struct CollectionSpec {
    pub initial_titles: Vec<String>,
    pub append_from_text_input: bool,
    pub toggle_all_from_checkbox: bool,
    pub clear_completed_from_button: bool,
    pub filters: Vec<String>,
    pub item_checkbox_toggle: bool,
    pub item_remove_button: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TableSpec {
    pub rows: usize,
    pub columns: usize,
    pub editor_source_family: String,
    pub formula_functions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlayfieldSpec {
    pub frame_event_path: String,
    pub control_event_path: String,
    pub arena_width: i64,
    pub arena_height: i64,
    pub ball: BallSpec,
    pub player: PaddleSpec,
    pub opponent: Option<PaddleSpec>,
    pub bricks: Option<BrickFieldSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BallSpec {
    pub x: i64,
    pub y: i64,
    pub dx: i64,
    pub dy: i64,
    pub size: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PaddleSpec {
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
pub struct BrickFieldSpec {
    pub rows: usize,
    pub columns: usize,
    pub top: i64,
    pub margin: i64,
    pub gap: i64,
    pub height: i64,
    pub score_per_hit: i64,
}

pub fn compile_source(name: &str, source: &str) -> Result<CompiledModule> {
    let parsed = parse_module(name, source)?;
    let hir = lower(parsed.clone());
    let dynamic_list_root = infer_dynamic_list_root(source);
    let dynamic_grid_root = infer_grid_source_root(source);
    let host_bindings = collect_host_bindings(
        source,
        dynamic_list_root.as_deref(),
        dynamic_grid_root.as_deref(),
    );
    let contracts = element_contracts();
    let mut seen = HashSet::new();
    let mut entries = Vec::new();

    for leaf in &parsed.source_leaves {
        let source_path = normalize_source_path(&leaf.path, dynamic_list_root.as_deref());
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
    let program = program_spec(name, source, &sources);
    Ok(CompiledModule {
        name: name.to_string(),
        hir,
        sources,
        program,
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
    source: &str,
    dynamic_list_root: Option<&str>,
    dynamic_grid_root: Option<&str>,
) -> Vec<HostBinding> {
    let mut bindings = Vec::new();
    let lines = source.lines().collect::<Vec<_>>();
    for (line_idx, line) in lines.iter().enumerate() {
        let mut search_from = 0;
        while let Some(element_idx) = line[search_from..].find("Element/") {
            let absolute_idx = search_from + element_idx;
            let function = line[absolute_idx..]
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '/' || *ch == '_')
                .collect::<String>();
            if function.is_empty() {
                search_from = absolute_idx + "Element/".len();
                continue;
            }
            let block = call_block(&lines, line_idx, absolute_idx);
            for source_base in element_args(&block, dynamic_list_root, dynamic_grid_root) {
                bindings.push(HostBinding {
                    function: function.clone(),
                    source_base,
                });
            }
            search_from = absolute_idx + function.len();
        }
    }
    bindings
}

fn call_block(lines: &[&str], start_line: usize, start_col: usize) -> String {
    let mut block = String::new();
    let mut depth = 0isize;
    let mut saw_open = false;
    for line in &lines[start_line..] {
        let text = if block.is_empty() {
            line.get(start_col..).unwrap_or_default()
        } else {
            line
        };
        for ch in text.chars() {
            if ch == '(' {
                depth += 1;
                saw_open = true;
            } else if ch == ')' {
                depth -= 1;
            }
            block.push(ch);
            if saw_open && depth <= 0 {
                return block;
            }
        }
        block.push('\n');
    }
    block
}

fn element_args(
    block: &str,
    dynamic_list_root: Option<&str>,
    dynamic_grid_root: Option<&str>,
) -> Vec<String> {
    let mut args = Vec::new();
    let mut depth = 0isize;
    let mut idx = 0;
    while idx < block.len() {
        let ch = block[idx..].chars().next().unwrap_or_default();
        if ch == '(' {
            depth += 1;
            idx += ch.len_utf8();
            continue;
        }
        if ch == ')' {
            depth -= 1;
            idx += ch.len_utf8();
            continue;
        }
        if depth == 1 && block[idx..].starts_with("element:") {
            let rest = &block[idx + "element:".len()..];
            let expr = rest
                .trim_start()
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '.')
                .collect::<String>();
            if !expr.is_empty() {
                args.push(normalize_binding_expr(
                    &expr,
                    dynamic_list_root,
                    dynamic_grid_root,
                ));
            }
            idx += "element:".len();
            continue;
        }
        idx += ch.len_utf8();
    }
    args
}

fn normalize_binding_expr(
    expr: &str,
    dynamic_list_root: Option<&str>,
    dynamic_grid_root: Option<&str>,
) -> String {
    if let Some(tail) = expr.strip_prefix("item.sources.") {
        let root = dynamic_list_root.unwrap_or("items");
        format!("{root}[*].sources.{tail}")
    } else if let Some(tail) = expr.strip_prefix("cell.sources.") {
        let root = dynamic_grid_root.unwrap_or("table");
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

fn program_spec(name: &str, source: &str, sources: &SourceInventory) -> ProgramSpec {
    let has_grid_element = source.contains("Element/grid(");
    let dynamic_families = dynamic_source_families(sources);
    let list_family = (!has_grid_element)
        .then(|| dynamic_families.first().cloned())
        .flatten();
    let grid_family = has_grid_element
        .then(|| dynamic_families.first().cloned())
        .flatten();

    let collection = list_family.as_ref().map(|family| CollectionSpec {
        initial_titles: extract_initial_collection_titles(source),
        append_from_text_input: source.contains("|> List/append(")
            && static_source_with_producer(sources, "Element/text_input(element.text)").is_some(),
        toggle_all_from_checkbox: static_source_with_producer(
            sources,
            "Element/checkbox(element.event.click)",
        )
        .is_some(),
        clear_completed_from_button: static_source_with_producer(
            sources,
            "Element/button(element.event.press)",
        )
        .is_some(),
        filters: static_view_names(sources),
        item_checkbox_toggle: source_family_with_producer(
            sources,
            family,
            "Element/checkbox(element.event.click)",
        )
        .is_some(),
        item_remove_button: source_family_with_producer(
            sources,
            family,
            "Element/button(element.event.press)",
        )
        .is_some(),
    });
    let table = grid_family.as_ref().map(|family| TableSpec {
        rows: extract_range_to(source, "rows").unwrap_or(100),
        columns: extract_range_to(source, "columns").unwrap_or(26),
        editor_source_family: source_family_with_producer(
            sources,
            family,
            "Element/text_input(element.text)",
        )
        .and_then(|path| path.strip_suffix(".text").map(str::to_string))
        .unwrap_or_else(|| format!("{family}.sources.editor")),
        formula_functions: extract_formula_functions(source),
    });
    let scalar_counter =
        static_source_with_producer(sources, "Element/button(element.event.press)")
            .filter(|_| collection.is_none() && table.is_none() && !source.contains("\nmotion:"))
            .map(|event_path| AccumulatorSpec {
                event_path,
                state_path: "counter".to_string(),
                initial: 0,
                step: extract_hold_increment(source, "counter").unwrap_or(0),
                button_label: extract_text_record_field(source, "button", "label")
                    .unwrap_or_else(|| "Increment".to_string()),
            });
    let timer_counter = static_tick_source(sources)
        .filter(|_| scalar_counter.is_none() && !source.contains("\nmotion:"))
        .map(|event_path| ClockAccumulatorSpec {
            event_path,
            state_path: "interval_count".to_string(),
            quantum_ms: 1000,
        });
    let playfield = source
        .contains("\nmotion:")
        .then(|| playfield_spec(source, sources));
    let title = extract_text_record_field(source, "game", "title")
        .unwrap_or_else(|| name.replace('_', " "));
    let scene = if collection.is_some() {
        SurfaceKind::Collection
    } else if table.is_some() {
        SurfaceKind::Table
    } else if scalar_counter.is_some() {
        SurfaceKind::ActionValue
    } else if timer_counter.is_some() {
        SurfaceKind::ClockValue
    } else if playfield.is_some() {
        SurfaceKind::Playfield
    } else {
        SurfaceKind::Blank
    };
    ProgramSpec {
        title,
        scene,
        scalar_counter,
        timer_counter,
        collection,
        table,
        playfield,
        physical_debug: source.contains("physical_debug: True"),
    }
}

fn playfield_spec(source: &str, sources: &SourceInventory) -> PlayfieldSpec {
    let playfield = named_block(source, "motion").unwrap_or_default();
    let arena = named_block(&playfield, "arena").unwrap_or_default();
    let ball = named_block(&playfield, "ball").unwrap_or_default();
    let player = named_block(&playfield, "player").unwrap_or_default();
    let opponent = named_block(&playfield, "opponent");
    let bricks = named_block(&playfield, "bricks");
    PlayfieldSpec {
        frame_event_path: first_static_path_matching(sources, ".event.frame").unwrap_or_default(),
        control_event_path: first_static_path_matching(sources, ".event.key_down.key")
            .unwrap_or_default(),
        arena_width: number_field(&arena, "width").unwrap_or(1000),
        arena_height: number_field(&arena, "height").unwrap_or(700),
        ball: BallSpec {
            x: number_field(&ball, "x").unwrap_or(500),
            y: number_field(&ball, "y").unwrap_or(350),
            dx: number_field(&ball, "dx").unwrap_or(10),
            dy: number_field(&ball, "dy").unwrap_or(8),
            size: number_field(&ball, "size").unwrap_or(22),
        },
        player: paddle_spec(
            &player,
            ControlAxis::Vertical,
            PaddleSpec {
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
        opponent: opponent.as_deref().map(|block| {
            paddle_spec(
                block,
                ControlAxis::Vertical,
                PaddleSpec {
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
        bricks: bricks.as_deref().map(|block| BrickFieldSpec {
            rows: number_field(block, "rows").unwrap_or(6).max(0) as usize,
            columns: number_field(block, "columns").unwrap_or(12).max(0) as usize,
            top: number_field(block, "top").unwrap_or(56),
            margin: number_field(block, "margin").unwrap_or(36),
            gap: number_field(block, "gap").unwrap_or(8),
            height: number_field(block, "height").unwrap_or(28),
            score_per_hit: number_field(block, "score_per_hit").unwrap_or(10),
        }),
    }
}

fn paddle_spec(block: &str, default_axis: ControlAxis, default: PaddleSpec) -> PaddleSpec {
    PaddleSpec {
        axis: axis_field(block, "axis").unwrap_or(default_axis),
        position: number_field(block, "position").unwrap_or(default.position),
        step: number_field(block, "step").unwrap_or(default.step),
        x: number_field(block, "x").unwrap_or(default.x),
        y: number_field(block, "y").unwrap_or(default.y),
        width: number_field(block, "width").unwrap_or(default.width),
        height: number_field(block, "height").unwrap_or(default.height),
        auto_track: tag_field(block, "auto_track").unwrap_or(default.auto_track),
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

fn static_view_names(sources: &SourceInventory) -> Vec<String> {
    sources
        .entries
        .iter()
        .filter(|entry| {
            matches!(&entry.owner, SourceOwner::Static)
                && entry.producer == "Element/button(element.event.press)"
        })
        .filter_map(|entry| {
            entry
                .path
                .strip_suffix(".event.press")
                .and_then(|base| base.rsplit('.').next())
                .and_then(|name| name.strip_prefix("filter_"))
                .map(str::to_string)
        })
        .collect()
}

fn infer_dynamic_list_root(source: &str) -> Option<String> {
    top_level_blocks(source)
        .into_iter()
        .find(|(_, block)| block.contains("LIST {") && block.contains("|> List/append("))
        .map(|(name, _)| name)
}

fn infer_grid_source_root(source: &str) -> Option<String> {
    source.contains("Element/grid(").then_some(())?;
    top_level_blocks(source)
        .into_iter()
        .find(|(name, block)| {
            name != "store" && block.lines().any(|line| line.trim() == "sources:")
        })
        .map(|(name, _)| name)
}

fn top_level_blocks(source: &str) -> Vec<(String, String)> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut idx = 0;
    while idx < lines.len() {
        let line = lines[idx];
        if line.starts_with(' ') || line.trim().is_empty() || line.trim_start().starts_with('#') {
            idx += 1;
            continue;
        }
        let Some((name, _)) = line.trim().split_once(':') else {
            idx += 1;
            continue;
        };
        if !is_plain_identifier(name.trim()) {
            idx += 1;
            continue;
        }
        let start = idx;
        idx += 1;
        while idx < lines.len() && (lines[idx].starts_with(' ') || lines[idx].trim().is_empty()) {
            idx += 1;
        }
        blocks.push((name.trim().to_string(), lines[start..idx].join("\n")));
    }
    blocks
}

fn is_plain_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn normalize_source_path(path: &str, dynamic_list_root: Option<&str>) -> String {
    if path.starts_with("store.sources.") {
        path.to_string()
    } else if let Some(tail) = path.strip_prefix("sources.") {
        let root = dynamic_list_root.unwrap_or("items");
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
        .map(|(root, _)| format!("{root} item"))
        .unwrap_or_else(|| "dynamic item".to_string())
}

fn extract_initial_collection_titles(source: &str) -> Vec<String> {
    let mut titles = Vec::new();
    let list_start = source.find("LIST {").unwrap_or(0);
    let list_end = source[list_start..]
        .find("\n    }")
        .map(|idx| list_start + idx)
        .unwrap_or(source.len());
    let mut rest = &source[list_start..list_end];
    let needle = "TEXT {";
    while let Some(idx) = rest.find(needle) {
        rest = &rest[idx + needle.len()..];
        let Some(end) = rest.find('}') else {
            break;
        };
        titles.push(rest[..end].trim().to_string());
        rest = &rest[end + 1..];
    }
    titles
}

fn extract_range_to(source: &str, binding: &str) -> Option<usize> {
    let binding_idx = source.find(&format!("{binding}:"))?;
    let rest = &source[binding_idx..source.len().min(binding_idx + 160)];
    let to_idx = rest.find("to:")?;
    let rest = &rest[to_idx + "to:".len()..];
    let digits = rest
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn extract_hold_increment(source: &str, binding: &str) -> Option<i64> {
    let binding_idx = source.find(&format!("{binding}:"))?;
    let block = &source[binding_idx..source.len().min(binding_idx + 240)];
    let state_idx = block.find("state +")?;
    let rest = &block[state_idx + "state +".len()..];
    let digits = rest
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
        .collect::<String>();
    digits.parse().ok()
}

fn extract_text_record_field(source: &str, record: &str, field: &str) -> Option<String> {
    let record_idx = source.find(&format!("{record}:"))?;
    let field_idx = source[record_idx..].find(&format!("{field}: TEXT {{"))? + record_idx;
    let rest = &source[field_idx + format!("{field}: TEXT {{").len()..];
    let end = rest.find('}')?;
    Some(rest[..end].trim().to_string())
}

fn named_block(source: &str, name: &str) -> Option<String> {
    let header = format!("{name}:");
    let lines = source.lines().collect::<Vec<_>>();
    let start = lines.iter().position(|line| line.trim_start() == header)?;
    let base_indent = lines[start].chars().take_while(|ch| *ch == ' ').count();
    let mut block = Vec::new();
    for line in lines.iter().skip(start + 1) {
        if line.trim().is_empty() {
            block.push(String::new());
            continue;
        }
        let indent = line.chars().take_while(|ch| *ch == ' ').count();
        if indent <= base_indent {
            break;
        }
        block.push((*line).to_string());
    }
    Some(block.join("\n"))
}

fn number_field(block: &str, field: &str) -> Option<i64> {
    let marker = format!("{field}:");
    for line in block.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix(&marker) else {
            continue;
        };
        let value = rest
            .trim_start()
            .chars()
            .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
            .collect::<String>();
        return value.parse().ok();
    }
    None
}

fn axis_field(block: &str, field: &str) -> Option<ControlAxis> {
    let marker = format!("{field}:");
    for line in block.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix(&marker) else {
            continue;
        };
        return match rest.trim() {
            "Horizontal" => Some(ControlAxis::Horizontal),
            "Vertical" => Some(ControlAxis::Vertical),
            _ => None,
        };
    }
    None
}

fn tag_field(block: &str, field: &str) -> Option<bool> {
    let marker = format!("{field}:");
    for line in block.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix(&marker) else {
            continue;
        };
        return match rest.trim() {
            "True" => Some(true),
            "False" => Some(false),
            _ => None,
        };
    }
    None
}

fn extract_formula_functions(source: &str) -> Vec<String> {
    let mut functions = Vec::new();
    let Some(formulas_idx) = source.find("formulas:") else {
        return functions;
    };
    for raw_line in source[formulas_idx..].lines().skip(1) {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "functions:" {
            continue;
        }
        let indent = raw_line.chars().take_while(|ch| *ch == ' ').count();
        if indent == 0 {
            break;
        }
        if let Some((name, module)) = trimmed.split_once(':') {
            let module = module.trim();
            if module.starts_with("Math/") {
                functions.push(name.trim().to_string());
            }
        }
    }
    functions
}
