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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ParsedModule {
    pub name: String,
    pub source_leaves: Vec<SourceLeaf>,
    pub module_calls: Vec<ModuleCall>,
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("invalid indentation at line {line}: tabs are not allowed")]
    TabIndent { line: usize },
}

pub fn parse_module(name: impl Into<String>, src: &str) -> Result<ParsedModule, ParseError> {
    let mut source_leaves = Vec::new();
    let mut module_calls = Vec::new();
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

        collect_module_calls(trimmed, line_no, &mut module_calls);
    }

    Ok(ParsedModule {
        name: name.into(),
        source_leaves,
        module_calls,
    })
}

fn is_plain_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '*')
}

fn collect_module_calls(line: &str, line_no: usize, calls: &mut Vec<ModuleCall>) {
    for token in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '/')) {
        if token.contains('/') && token.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            calls.push(ModuleCall {
                path: token.trim_matches('/').to_string(),
                span: Span {
                    line: line_no,
                    column: line.find(token).unwrap_or(0) + 1,
                },
            });
        }
    }
}
