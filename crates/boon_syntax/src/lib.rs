use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceLeaf {
    pub path: String,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModuleCall {
    pub path: String,
    pub span: Span,
    pub args: Vec<CallArg>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CallArg {
    pub name: String,
    pub value: String,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TextLiteral {
    pub value: String,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StateStep {
    pub state: String,
    pub amount: i64,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MapBinding {
    pub variable: String,
    pub collection: String,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ParsedLine {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ParsedRecordEntry {
    pub key: String,
    pub value: Option<String>,
    pub span: Span,
    pub children: Vec<ParsedRecordEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ParsedModule {
    pub name: String,
    pub lines: Vec<ParsedLine>,
    pub records: Vec<ParsedRecordEntry>,
    pub source_leaves: Vec<SourceLeaf>,
    pub module_calls: Vec<ModuleCall>,
    pub text_literals: Vec<TextLiteral>,
    pub state_steps: Vec<StateStep>,
    pub map_bindings: Vec<MapBinding>,
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("invalid indentation at line {line}: tabs are not allowed")]
    TabIndent { line: usize },
}

pub fn parse_module(name: impl Into<String>, src: &str) -> Result<ParsedModule, ParseError> {
    let mut lines = Vec::new();
    let mut record_lines = Vec::new();
    let mut source_leaves = Vec::new();
    let mut text_literals = Vec::new();
    let mut state_steps = Vec::new();
    let mut map_bindings = Vec::new();
    let mut key_stack: Vec<(usize, String)> = Vec::new();

    for (line_idx, raw_line) in src.lines().enumerate() {
        let line_no = line_idx + 1;
        if raw_line.starts_with('\t') {
            return Err(ParseError::TabIndent { line: line_no });
        }
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        lines.push(ParsedLine {
            text: trimmed.to_string(),
            span: Span {
                line: line_no,
                column: raw_line.find(trimmed).unwrap_or(0) + 1,
            },
        });

        let indent = raw_line.chars().take_while(|c| *c == ' ').count();
        while key_stack
            .last()
            .is_some_and(|(last_indent, _)| *last_indent >= indent)
        {
            key_stack.pop();
        }

        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim();
            if is_plain_key(key) {
                record_lines.push(RecordLine {
                    indent,
                    key: key.to_string(),
                    value: (!value.trim().is_empty()).then(|| value.trim().to_string()),
                    span: Span {
                        line: line_no,
                        column: raw_line.find(key).unwrap_or(0) + 1,
                    },
                });
                key_stack.push((indent, key.to_string()));
                if value.trim() == "SOURCE" {
                    let path = key_stack
                        .iter()
                        .map(|(_, key)| key.as_str())
                        .collect::<Vec<_>>()
                        .join(".");
                    let column = raw_line.find("SOURCE").unwrap_or(0) + 1;
                    source_leaves.push(SourceLeaf {
                        path,
                        span: Span {
                            line: line_no,
                            column,
                        },
                    });
                }
            }
        } else if trimmed == "SOURCE" {
            let path = key_stack
                .iter()
                .map(|(_, key)| key.as_str())
                .collect::<Vec<_>>()
                .join(".");
            let column = raw_line.find("SOURCE").unwrap_or(0) + 1;
            source_leaves.push(SourceLeaf {
                path,
                span: Span {
                    line: line_no,
                    column,
                },
            });
        }

        collect_text_literals(trimmed, line_no, &mut text_literals);
        collect_state_steps(trimmed, line_no, &mut state_steps);
        collect_map_bindings(trimmed, line_no, &mut map_bindings);
    }

    let module_calls = collect_module_calls(&lines);
    Ok(ParsedModule {
        name: name.into(),
        lines,
        records: parse_record_tree(&record_lines),
        source_leaves,
        module_calls,
        text_literals,
        state_steps,
        map_bindings,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RecordLine {
    indent: usize,
    key: String,
    value: Option<String>,
    span: Span,
}

fn parse_record_tree(lines: &[RecordLine]) -> Vec<ParsedRecordEntry> {
    let mut idx = 0;
    parse_record_level(lines, &mut idx, 0)
}

fn parse_record_level(
    lines: &[RecordLine],
    idx: &mut usize,
    level_indent: usize,
) -> Vec<ParsedRecordEntry> {
    let mut entries = Vec::new();
    while *idx < lines.len() {
        let line = &lines[*idx];
        if line.indent < level_indent {
            break;
        }
        if line.indent > level_indent {
            break;
        }
        *idx += 1;
        let children = if *idx < lines.len() && lines[*idx].indent > line.indent {
            let child_indent = lines[*idx].indent;
            parse_record_level(lines, idx, child_indent)
        } else {
            Vec::new()
        };
        entries.push(ParsedRecordEntry {
            key: line.key.clone(),
            value: line.value.clone(),
            span: line.span,
            children,
        });
    }
    entries
}

fn is_plain_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '*')
}

fn collect_text_literals(line: &str, line_no: usize, literals: &mut Vec<TextLiteral>) {
    let mut rest = line;
    let mut offset = 0;
    while let Some(idx) = rest.find("TEXT {") {
        let start = offset + idx + "TEXT {".len();
        let after = &line[start..];
        let Some(end) = after.find('}') else {
            break;
        };
        literals.push(TextLiteral {
            value: after[..end].trim().to_string(),
            span: Span {
                line: line_no,
                column: offset + idx + 1,
            },
        });
        offset = start + end + 1;
        rest = &line[offset..];
    }
}

fn collect_state_steps(line: &str, line_no: usize, steps: &mut Vec<StateStep>) {
    let bytes = line.as_bytes();
    for plus in line.match_indices('+').map(|(idx, _)| idx) {
        let mut left_end = plus;
        while left_end > 0 && bytes[left_end - 1].is_ascii_whitespace() {
            left_end -= 1;
        }
        let mut left_start = left_end;
        while left_start > 0 {
            let byte = bytes[left_start - 1];
            if byte.is_ascii_alphanumeric() || byte == b'_' {
                left_start -= 1;
            } else {
                break;
            }
        }
        if left_start == left_end {
            continue;
        }
        let mut right_start = plus + 1;
        while right_start < bytes.len() && bytes[right_start].is_ascii_whitespace() {
            right_start += 1;
        }
        let mut right_end = right_start;
        if right_end < bytes.len() && bytes[right_end] == b'-' {
            right_end += 1;
        }
        while right_end < bytes.len() && bytes[right_end].is_ascii_digit() {
            right_end += 1;
        }
        if right_start == right_end {
            continue;
        }
        if let Ok(amount) = line[right_start..right_end].parse() {
            steps.push(StateStep {
                state: line[left_start..left_end].to_string(),
                amount,
                span: Span {
                    line: line_no,
                    column: left_start + 1,
                },
            });
        }
    }
}

fn collect_map_bindings(line: &str, line_no: usize, bindings: &mut Vec<MapBinding>) {
    let mut rest = line;
    let mut offset = 0;
    while let Some(idx) = rest.find("List/map(") {
        let collection = rest[..idx]
            .trim_end()
            .strip_suffix("|>")
            .unwrap_or(rest[..idx].trim_end())
            .trim()
            .to_string();
        let start = offset + idx + "List/map(".len();
        let after = &line[start..];
        let variable = after
            .trim_start()
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect::<String>();
        if !variable.is_empty() {
            let leading_ws = after.len() - after.trim_start().len();
            bindings.push(MapBinding {
                variable,
                collection,
                span: Span {
                    line: line_no,
                    column: start + leading_ws + 1,
                },
            });
        }
        offset = start;
        rest = &line[offset..];
    }
}

fn collect_module_calls(lines: &[ParsedLine]) -> Vec<ModuleCall> {
    let line_text = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>();
    let mut calls = Vec::new();
    for (line_idx, line) in line_text.iter().enumerate() {
        let mut search_from = 0;
        while search_from < line.len() {
            let Some((relative_idx, token)) = next_module_token(&line[search_from..]) else {
                break;
            };
            let absolute_idx = search_from + relative_idx;
            let block = call_block(&line_text, line_idx, absolute_idx + token.len());
            calls.push(ModuleCall {
                path: token.to_string(),
                span: Span {
                    line: lines[line_idx].span.line,
                    column: lines[line_idx].span.column + absolute_idx,
                },
                args: parse_call_args(&block, lines[line_idx].span.line),
            });
            search_from = absolute_idx + token.len();
        }
    }
    calls
}

fn next_module_token(line: &str) -> Option<(usize, &str)> {
    let mut start = None;
    for (idx, ch) in line.char_indices() {
        if start.is_none() && ch.is_ascii_uppercase() {
            start = Some(idx);
        }
        let Some(token_start) = start else {
            continue;
        };
        let valid = ch.is_ascii_alphanumeric() || ch == '_' || ch == '/';
        if !valid {
            let token = &line[token_start..idx];
            if token.contains('/') {
                return Some((token_start, token.trim_matches('/')));
            }
            start = None;
        }
    }
    if let Some(token_start) = start {
        let token = &line[token_start..];
        if token.contains('/') {
            return Some((token_start, token.trim_matches('/')));
        }
    }
    None
}

fn call_block(lines: &[&str], start_line: usize, after_token_col: usize) -> String {
    let mut block = String::new();
    let mut depth = 0isize;
    let mut saw_open = false;
    for line in &lines[start_line..] {
        let text = if block.is_empty() {
            line.get(after_token_col..).unwrap_or_default()
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

fn parse_call_args(block: &str, line_no: usize) -> Vec<CallArg> {
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
        if depth == 1 && (ch.is_ascii_alphabetic() || ch == '_') {
            let name_start = idx;
            let name = block[name_start..]
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                .collect::<String>();
            let after_name = name_start + name.len();
            let rest = block[after_name..].trim_start();
            if let Some(after_colon) = rest.strip_prefix(':') {
                let value_start = block.len() - after_colon.len();
                let value = after_colon
                    .trim_start()
                    .chars()
                    .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '.')
                    .collect::<String>();
                if !value.is_empty() {
                    args.push(CallArg {
                        name,
                        value,
                        span: Span {
                            line: line_no,
                            column: name_start + 1,
                        },
                    });
                }
                idx = value_start;
                continue;
            }
        }
        idx += ch.len_utf8();
    }
    args
}
