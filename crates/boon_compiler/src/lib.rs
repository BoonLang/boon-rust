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
    pub counter: bool,
    pub interval: bool,
    pub keyed_list: Option<KeyedListSpec>,
    pub grid: Option<GridSpec>,
    pub frame_counter: bool,
    pub physical_debug: bool,
    pub title: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct KeyedListSpec {
    pub initial_titles: Vec<String>,
    pub append_from_text_input: bool,
    pub toggle_all_from_checkbox: bool,
    pub clear_completed_from_button: bool,
    pub filters: Vec<String>,
    pub item_checkbox_toggle: bool,
    pub item_remove_button: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GridSpec {
    pub rows: usize,
    pub columns: usize,
    pub editor_source_family: String,
    pub formula_functions: Vec<String>,
}

pub fn compile_source(name: &str, source: &str) -> Result<CompiledModule> {
    let parsed = parse_module(name, source)?;
    let hir = lower(parsed.clone());
    let host_bindings = collect_host_bindings(source);
    let contracts = element_contracts();
    let mut seen = HashSet::new();
    let mut entries = Vec::new();

    for leaf in &parsed.source_leaves {
        let source_path = normalize_source_path(&leaf.path);
        if source_path.is_empty() {
            bail!("SOURCE at line {} has no data path", leaf.span.line);
        }
        if !seen.insert(source_path.clone()) {
            bail!("SOURCE path `{}` is declared more than once", source_path);
        }
        let binding = binding_for_source(&source_path, &host_bindings, &contracts)?;
        let shape = binding.shape;
        let owner = if source_path.contains("[*]")
            || source_path.starts_with("todos.")
            || source_path.starts_with("cells.")
        {
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

fn collect_host_bindings(source: &str) -> Vec<HostBinding> {
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
            for source_base in element_args(&block) {
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

fn element_args(block: &str) -> Vec<String> {
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
                args.push(normalize_binding_expr(&expr));
            }
            idx += "element:".len();
            continue;
        }
        idx += ch.len_utf8();
    }
    args
}

fn normalize_binding_expr(expr: &str) -> String {
    if let Some(tail) = expr.strip_prefix("item.sources.") {
        format!("todos[*].sources.{tail}")
    } else if let Some(tail) = expr.strip_prefix("cell.sources.") {
        format!("cells[*].sources.{tail}")
    } else if let Some(tail) = expr.strip_prefix("cells.sources.") {
        format!("cells[*].sources.{tail}")
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
    let has_source = |path: &str| sources.entries.iter().any(|entry| entry.path == path);
    let keyed_list = sources
        .entries
        .iter()
        .any(|entry| entry.path.starts_with("todos[*].sources."))
        .then(|| KeyedListSpec {
            initial_titles: extract_initial_keyed_list_titles(source),
            append_from_text_input: source.contains("|> List/append(")
                && source.contains("new_todo_input"),
            toggle_all_from_checkbox: source.contains("toggle_all_checkbox.event.click"),
            clear_completed_from_button: source.contains("clear_completed_button.event.press"),
            filters: ["all", "active", "completed"]
                .into_iter()
                .filter(|filter| source.contains(&format!("filter_{filter}")))
                .map(str::to_string)
                .collect(),
            item_checkbox_toggle: source.contains("sources.checkbox.event.click"),
            item_remove_button: source.contains("sources.remove_button.event.press"),
        });
    let grid = sources
        .entries
        .iter()
        .any(|entry| entry.path.starts_with("cells[*].sources."))
        .then(|| GridSpec {
            rows: extract_range_to(source, "rows").unwrap_or(100),
            columns: extract_range_to(source, "columns").unwrap_or(26),
            editor_source_family: "cells[*].sources.editor".to_string(),
            formula_functions: extract_formula_functions(source),
        });
    let title = extract_text_record_field(source, "game", "title")
        .unwrap_or_else(|| name.replace('_', " "));
    ProgramSpec {
        counter: has_source("store.sources.increment_button.event.press"),
        interval: has_source("store.sources.clock.event.tick"),
        keyed_list,
        grid,
        frame_counter: has_source("store.sources.tick.event.frame"),
        physical_debug: source.contains("physical_debug: True"),
        title,
    }
}

fn normalize_source_path(path: &str) -> String {
    if path.starts_with("sources.") {
        format!("todos[*].{path}")
    } else if path.starts_with("cells.sources.") {
        path.replacen("cells.sources.", "cells[*].sources.", 1)
    } else {
        path.to_string()
    }
}

fn owner_path(path: &str) -> String {
    if path.starts_with("cells.") {
        "cells item".to_string()
    } else if path.contains("todos") {
        "store.todos item".to_string()
    } else {
        "dynamic item".to_string()
    }
}

fn extract_initial_keyed_list_titles(source: &str) -> Vec<String> {
    let mut titles = Vec::new();
    let needle = "new_todo(title: TEXT {";
    let mut rest = source;
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

fn extract_text_record_field(source: &str, record: &str, field: &str) -> Option<String> {
    let record_idx = source.find(&format!("{record}:"))?;
    let field_idx = source[record_idx..].find(&format!("{field}: TEXT {{"))? + record_idx;
    let rest = &source[field_idx + format!("{field}: TEXT {{").len()..];
    let end = rest.find('}')?;
    Some(rest[..end].trim().to_string())
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
