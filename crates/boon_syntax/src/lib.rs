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
    pub ast: AstModule,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AstModule {
    pub name: String,
    pub items: Vec<AstItem>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AstItem {
    Record(AstRecord),
    Function(AstFunction),
    Expression(AstExpr),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AstRecord {
    pub key: String,
    pub value: Option<AstExpr>,
    pub span: Span,
    pub children: Vec<AstRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AstFunction {
    pub name: String,
    pub params: Vec<String>,
    pub body: AstExpr,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AstExpr {
    pub kind: AstExprKind,
    pub span: Span,
    pub raw: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AstExprKind {
    Source,
    Number {
        value: i64,
    },
    Bool {
        value: bool,
    },
    Tag {
        value: String,
    },
    Text {
        value: String,
    },
    Path {
        value: String,
    },
    Passed,
    Skip,
    Record {
        entries: Vec<AstRecord>,
    },
    List {
        items: Vec<AstExpr>,
    },
    Block {
        bindings: Vec<AstBinding>,
    },
    When {
        arms: Vec<AstWhenArm>,
    },
    Then {
        body: Box<AstExpr>,
    },
    Hold {
        state: String,
        body: Box<AstExpr>,
    },
    Latest {
        branches: Vec<AstExpr>,
    },
    Call {
        path: String,
        args: Vec<AstCallArg>,
    },
    Pipeline {
        input: Box<AstExpr>,
        stages: Vec<AstExpr>,
    },
    Binary {
        op: AstBinaryOp,
        left: Box<AstExpr>,
        right: Box<AstExpr>,
    },
    Raw,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AstBinding {
    pub name: String,
    pub value: AstExpr,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AstCallArg {
    pub name: String,
    pub value: AstExpr,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AstWhenArm {
    pub pattern: String,
    pub value: AstExpr,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AstBinaryOp {
    Add,
    Subtract,
    Equal,
}

pub fn parse_module(name: impl Into<String>, src: &str) -> Result<ParsedModule, ParseError> {
    let name = name.into();
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
    let records = parse_record_tree(&record_lines);
    let ast = build_ast(&name, &lines, &records);
    Ok(ParsedModule {
        name,
        ast,
        lines,
        records,
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

fn build_ast(name: &str, lines: &[ParsedLine], records: &[ParsedRecordEntry]) -> AstModule {
    let functions = collect_ast_functions(lines);
    let function_lines = functions
        .iter()
        .map(|function| function.span.line)
        .collect::<std::collections::HashSet<_>>();
    let mut items = records
        .iter()
        .cloned()
        .map(|record| AstItem::Record(ast_record(record)))
        .collect::<Vec<_>>();
    items.extend(functions.into_iter().map(AstItem::Function));
    items.extend(
        collect_ast_expression_blocks(lines, &function_lines)
            .into_iter()
            .map(AstItem::Expression),
    );
    AstModule {
        name: name.to_string(),
        items,
    }
}

fn ast_record(record: ParsedRecordEntry) -> AstRecord {
    let value = record.value.as_ref().map(|value| {
        parse_expr(
            value,
            Span {
                line: record.span.line,
                column: record.span.column + record.key.len() + 1,
            },
        )
    });
    AstRecord {
        key: record.key,
        value,
        span: record.span,
        children: record.children.into_iter().map(ast_record).collect(),
    }
}

fn collect_ast_functions(lines: &[ParsedLine]) -> Vec<AstFunction> {
    let mut functions = Vec::new();
    let mut idx = 0;
    while idx < lines.len() {
        let line = &lines[idx];
        let text = line.text.trim();
        let Some(signature) = text.strip_prefix("FUNCTION ") else {
            idx += 1;
            continue;
        };
        let Some(open_paren) = signature.find('(') else {
            idx += 1;
            continue;
        };
        let Some(close_paren) = signature[open_paren + 1..].find(')') else {
            idx += 1;
            continue;
        };
        let name = signature[..open_paren].trim().to_string();
        if !is_identifier_path(&name) {
            idx += 1;
            continue;
        }
        let params_text = &signature[open_paren + 1..open_paren + 1 + close_paren];
        let params = params_text
            .split(',')
            .map(str::trim)
            .filter(|param| !param.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut block = String::new();
        let mut depth = brace_delta(text);
        let mut cursor = idx + 1;
        while cursor < lines.len() {
            let body_line = lines[cursor].text.as_str();
            depth += brace_delta(body_line);
            block.push_str(body_line);
            block.push('\n');
            cursor += 1;
            if depth <= 0 {
                break;
            }
        }
        let body = trim_wrapping_braces(&block);
        functions.push(AstFunction {
            name,
            params,
            body: parse_expr(body.trim(), line.span),
            span: line.span,
        });
        idx = cursor;
    }
    functions
}

fn collect_ast_expression_blocks(
    lines: &[ParsedLine],
    function_lines: &std::collections::HashSet<usize>,
) -> Vec<AstExpr> {
    let mut expressions = Vec::new();
    let mut idx = 0;
    while idx < lines.len() {
        if function_lines.contains(&lines[idx].span.line) {
            idx += 1;
            continue;
        }
        let text = lines[idx].text.trim();
        let starts_expression = is_expression_start(text)
            || lines
                .get(idx + 1)
                .is_some_and(|next| next.text.trim_start().starts_with("|>"));
        if !starts_expression || is_plain_record_line(text) {
            idx += 1;
            continue;
        }
        let span = lines[idx].span;
        let mut raw = String::new();
        let mut depth = 0isize;
        let mut cursor = idx;
        while cursor < lines.len() {
            let line = lines[cursor].text.trim();
            if cursor > idx
                && depth <= 0
                && is_plain_record_line(line)
                && !line.starts_with("|>")
                && !line.contains("=>")
            {
                break;
            }
            raw.push_str(line);
            raw.push('\n');
            depth += delimiter_delta(line);
            cursor += 1;
            if depth <= 0
                && !lines
                    .get(cursor)
                    .is_some_and(|next| next.text.trim_start().starts_with("|>"))
            {
                break;
            }
        }
        expressions.push(parse_expr(raw.trim(), span));
        idx = cursor.max(idx + 1);
    }
    expressions
}

fn is_expression_start(text: &str) -> bool {
    text.contains("|>")
        || text.starts_with("LIST {")
        || text.starts_with("BLOCK {")
        || text.starts_with("LATEST {")
        || text.starts_with("WHEN {")
        || text.starts_with("THEN {")
        || text.starts_with("Document/")
        || text.starts_with("Element/")
}

fn is_plain_record_line(text: &str) -> bool {
    let Some((key, _)) = text.split_once(':') else {
        return false;
    };
    is_plain_key(key.trim())
}

fn brace_delta(text: &str) -> isize {
    text.chars().fold(0, |depth, ch| match ch {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}

fn delimiter_delta(text: &str) -> isize {
    text.chars().fold(0, |depth, ch| match ch {
        '{' | '(' | '[' => depth + 1,
        '}' | ')' | ']' => depth - 1,
        _ => depth,
    })
}

fn trim_wrapping_braces(text: &str) -> &str {
    let trimmed = text.trim();
    trimmed
        .strip_suffix('}')
        .unwrap_or(trimmed)
        .trim()
        .strip_prefix('{')
        .unwrap_or_else(|| trimmed.strip_suffix('}').unwrap_or(trimmed).trim())
        .trim()
}

fn parse_expr(raw: &str, span: Span) -> AstExpr {
    let raw = raw.trim();
    let pipeline = split_top_level_pipeline(raw);
    if pipeline.len() > 1 {
        return AstExpr {
            kind: AstExprKind::Pipeline {
                input: Box::new(parse_expr(pipeline[0], span)),
                stages: pipeline[1..]
                    .iter()
                    .map(|stage| parse_expr(stage, span))
                    .collect(),
            },
            span,
            raw: raw.to_string(),
        };
    }
    if let Some((left, op, right)) = split_top_level_binary(raw) {
        return AstExpr {
            kind: AstExprKind::Binary {
                op,
                left: Box::new(parse_expr(left, span)),
                right: Box::new(parse_expr(right, span)),
            },
            span,
            raw: raw.to_string(),
        };
    }
    let kind = if raw == "SOURCE" {
        AstExprKind::Source
    } else if raw == "PASSED" {
        AstExprKind::Passed
    } else if raw == "SKIP" {
        AstExprKind::Skip
    } else if raw == "True" || raw == "False" {
        AstExprKind::Bool {
            value: raw == "True",
        }
    } else if let Ok(value) = raw.parse::<i64>() {
        AstExprKind::Number { value }
    } else if let Some(value) = parse_text_literal_expr(raw) {
        AstExprKind::Text { value }
    } else if let Some((state, body)) = parse_wrapped_keyword(raw, "HOLD") {
        AstExprKind::Hold {
            state: state.trim().to_string(),
            body: Box::new(parse_expr(body, span)),
        }
    } else if let Some((_, body)) = parse_wrapped_keyword(raw, "THEN") {
        AstExprKind::Then {
            body: Box::new(parse_expr(body, span)),
        }
    } else if let Some((_, body)) = parse_wrapped_keyword(raw, "WHEN") {
        AstExprKind::When {
            arms: parse_when_arms(body, span),
        }
    } else if let Some((_, body)) = parse_wrapped_keyword(raw, "LATEST") {
        AstExprKind::Latest {
            branches: parse_expression_lines(body, span),
        }
    } else if let Some((_, body)) = parse_wrapped_keyword(raw, "BLOCK") {
        AstExprKind::Block {
            bindings: parse_bindings(body, span),
        }
    } else if let Some((_, body)) = parse_wrapped_keyword(raw, "LIST") {
        AstExprKind::List {
            items: parse_expression_lines(body, span),
        }
    } else if let Some(body) = parse_record_literal_body(raw) {
        AstExprKind::Record {
            entries: parse_record_literal_entries(body, span),
        }
    } else if let Some((path, args)) = parse_call_expr(raw, span) {
        AstExprKind::Call { path, args }
    } else if is_path_like(raw) || is_local_path_like(raw) {
        AstExprKind::Path {
            value: raw.to_string(),
        }
    } else if is_tag_like(raw) {
        AstExprKind::Tag {
            value: raw.to_string(),
        }
    } else {
        AstExprKind::Raw
    };
    AstExpr {
        kind,
        span,
        raw: raw.to_string(),
    }
}

fn parse_text_literal_expr(raw: &str) -> Option<String> {
    let body = raw.strip_prefix("TEXT {")?.strip_suffix('}')?;
    Some(body.trim().to_string())
}

fn parse_wrapped_keyword<'a>(raw: &'a str, keyword: &str) -> Option<(&'a str, &'a str)> {
    let rest = raw.strip_prefix(keyword)?.trim_start();
    let open = rest.find('{')?;
    let prefix = rest[..open].trim();
    let body = matching_wrapped_body(&rest[open..])?;
    Some((prefix, body))
}

fn matching_wrapped_body(raw: &str) -> Option<&str> {
    let raw = raw.trim();
    if !raw.starts_with('{') {
        return None;
    }
    let close = matching_delimiter(raw, 0, '{', '}')?;
    Some(raw[1..close].trim())
}

fn parse_record_literal_body(raw: &str) -> Option<&str> {
    let raw = raw.trim();
    if !raw.starts_with('[') {
        return None;
    }
    let close = matching_delimiter(raw, 0, '[', ']')?;
    Some(raw[1..close].trim())
}

fn matching_delimiter(raw: &str, open_idx: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0isize;
    for (idx, ch) in raw.char_indices().skip_while(|(idx, _)| *idx < open_idx) {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

fn parse_expression_lines(body: &str, span: Span) -> Vec<AstExpr> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| *line != "{" && *line != "}")
        .map(|line| parse_expr(line.trim_end_matches(',').trim(), span))
        .collect()
}

fn parse_bindings(body: &str, span: Span) -> Vec<AstBinding> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some(AstBinding {
                name: name.trim().to_string(),
                value: parse_expr(value.trim(), span),
                span,
            })
        })
        .collect()
}

fn parse_record_literal_entries(body: &str, span: Span) -> Vec<AstRecord> {
    parse_bindings(body, span)
        .into_iter()
        .map(|binding| AstRecord {
            key: binding.name,
            value: Some(binding.value),
            span: binding.span,
            children: Vec::new(),
        })
        .collect()
}

fn parse_when_arms(body: &str, span: Span) -> Vec<AstWhenArm> {
    split_top_level_items(body)
        .into_iter()
        .flat_map(|item| {
            parse_when_item_arms(item, span)
                .into_iter()
                .collect::<Vec<_>>()
        })
        .collect()
}

fn parse_when_item_arms(item: &str, span: Span) -> Vec<AstWhenArm> {
    if let Some((pattern, value)) = item.split_once("=>")
        && matches!(
            value.split_whitespace().next(),
            Some("BLOCK" | "THEN" | "WHEN" | "LATEST" | "LIST")
        )
    {
        return vec![AstWhenArm {
            pattern: pattern.trim().to_string(),
            value: parse_expr(value.trim(), span),
            span,
        }];
    }
    if item.matches("=>").count() <= 1 {
        return item
            .split_once("=>")
            .map(|(pattern, value)| {
                vec![AstWhenArm {
                    pattern: pattern.trim().to_string(),
                    value: parse_expr(value.trim(), span),
                    span,
                }]
            })
            .unwrap_or_default();
    }

    let tokens = item.split_whitespace().collect::<Vec<_>>();
    let mut arms = Vec::new();
    let mut idx = 0;
    while idx + 2 < tokens.len() {
        let pattern = tokens[idx];
        if tokens[idx + 1] != "=>" {
            break;
        }
        idx += 2;
        let value_start = idx;
        while idx + 1 < tokens.len() && tokens[idx + 1] != "=>" {
            idx += 1;
        }
        let value = tokens[value_start..idx].join(" ");
        if !pattern.is_empty() && !value.is_empty() {
            arms.push(AstWhenArm {
                pattern: pattern.to_string(),
                value: parse_expr(&value, span),
                span,
            });
        }
    }
    arms
}

fn parse_call_expr(raw: &str, span: Span) -> Option<(String, Vec<AstCallArg>)> {
    let open = raw.find('(')?;
    if !raw.ends_with(')') {
        return None;
    }
    let path = raw[..open].trim();
    if !is_identifier_path(path) {
        return None;
    }
    let close = matching_delimiter(raw, open, '(', ')')?;
    if close != raw.len() - 1 {
        return None;
    }
    let args_body = &raw[open + 1..close];
    let args = split_top_level_items(args_body)
        .into_iter()
        .filter_map(|item| {
            let (name, value) = item.split_once(':')?;
            Some(AstCallArg {
                name: name.trim().to_string(),
                value: parse_expr(value.trim(), span),
                span,
            })
        })
        .collect();
    Some((path.to_string(), args))
}

fn split_top_level_pipeline(raw: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = DelimiterDepth::default();
    let bytes = raw.as_bytes();
    let mut idx = 0;
    while idx + 1 < bytes.len() {
        let ch = raw[idx..].chars().next().unwrap_or_default();
        depth.update(ch);
        if depth.is_top_level() && bytes[idx] == b'|' && bytes[idx + 1] == b'>' {
            parts.push(raw[start..idx].trim());
            idx += 2;
            start = idx;
            continue;
        }
        idx += ch.len_utf8();
    }
    if parts.is_empty() {
        vec![raw]
    } else {
        parts.push(raw[start..].trim());
        parts
    }
}

fn split_top_level_binary(raw: &str) -> Option<(&str, AstBinaryOp, &str)> {
    for (needle, op) in [
        ("==", AstBinaryOp::Equal),
        ("+", AstBinaryOp::Add),
        ("-", AstBinaryOp::Subtract),
    ] {
        let mut depth = DelimiterDepth::default();
        let bytes = raw.as_bytes();
        let mut idx = 0;
        while idx < bytes.len() {
            let ch = raw[idx..].chars().next().unwrap_or_default();
            if depth.is_top_level()
                && raw[idx..].starts_with(needle)
                && idx > 0
                && idx + needle.len() < raw.len()
                && raw[..idx].trim().parse::<i64>().is_err()
            {
                let left = raw[..idx].trim();
                let right = raw[idx + needle.len()..].trim();
                if !left.is_empty() && !right.is_empty() {
                    return Some((left, op, right));
                }
            }
            depth.update(ch);
            idx += ch.len_utf8();
        }
    }
    None
}

fn split_top_level_items(raw: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = DelimiterDepth::default();
    for (idx, ch) in raw.char_indices() {
        if depth.is_top_level() && (ch == ',' || ch == '\n') {
            let item = raw[start..idx].trim();
            if !item.is_empty() {
                parts.push(item);
            }
            start = idx + ch.len_utf8();
            continue;
        }
        depth.update(ch);
    }
    let item = raw[start..].trim();
    if !item.is_empty() {
        parts.push(item);
    }
    parts
}

#[derive(Default)]
struct DelimiterDepth {
    paren: isize,
    brace: isize,
    bracket: isize,
}

impl DelimiterDepth {
    fn update(&mut self, ch: char) {
        match ch {
            '(' => self.paren += 1,
            ')' => self.paren -= 1,
            '{' => self.brace += 1,
            '}' => self.brace -= 1,
            '[' => self.bracket += 1,
            ']' => self.bracket -= 1,
            _ => {}
        }
    }

    fn is_top_level(&self) -> bool {
        self.paren == 0 && self.brace == 0 && self.bracket == 0
    }
}

fn is_path_like(raw: &str) -> bool {
    !raw.is_empty()
        && raw
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '/'))
        && (raw.contains('.') || raw.contains('/'))
}

fn is_local_path_like(raw: &str) -> bool {
    raw.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_lowercase() || ch == '_')
        && raw
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn is_tag_like(raw: &str) -> bool {
    raw == "__"
        || raw.chars().next().is_some_and(|ch| ch.is_ascii_uppercase())
            && raw
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn is_identifier_path(raw: &str) -> bool {
    !raw.is_empty()
        && raw
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '/'))
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn parses_core_ast_constructs_from_source() {
        let parsed = parse_module(
            "fixture",
            r#"
store:
    sources:
        button:
            event:
                press: SOURCE

FUNCTION build(title) {
    [
        title: title
    ]
}

counter:
    0 |> HOLD state {
        store.sources.button.event.press
        |> THEN { state + 1 }
    }

title:
    store.sources.button.event.press
    |> WHEN {
        Enter => BLOCK {
            label: TEXT { ok }
        }
        __ => SKIP
    }

items:
    LIST {
        build(title: TEXT { First })
    }
    |> List/map(item, new: item)
"#,
        )
        .expect("fixture parses");

        assert!(
            parsed.ast.items.iter().any(
                |item| matches!(item, AstItem::Function(function) if function.name == "build")
            )
        );
        assert!(
            contains_expr(&parsed.ast, |expr| matches!(expr.kind, AstExprKind::Source)),
            "SOURCE should be represented in AST"
        );
        assert!(
            contains_expr(&parsed.ast, |expr| matches!(
                expr.kind,
                AstExprKind::Hold { .. }
            )),
            "HOLD should be represented in AST"
        );
        assert!(
            contains_expr(&parsed.ast, |expr| matches!(
                expr.kind,
                AstExprKind::Then { .. }
            )),
            "THEN should be represented in AST"
        );
        assert!(
            contains_expr(&parsed.ast, |expr| matches!(
                expr.kind,
                AstExprKind::When { .. }
            )),
            "WHEN should be represented in AST"
        );
        assert!(
            contains_expr(&parsed.ast, |expr| matches!(
                expr.kind,
                AstExprKind::Block { .. }
            )),
            "BLOCK should be represented in AST"
        );
        assert!(
            contains_expr(&parsed.ast, |expr| matches!(
                expr.kind,
                AstExprKind::List { .. }
            )),
            "LIST should be represented in AST"
        );
        assert!(
            contains_expr(&parsed.ast, |expr| {
                matches!(&expr.kind, AstExprKind::Call { path, .. } if path == "List/map")
            }),
            "List/map should be represented as a call in AST"
        );
    }

    fn contains_expr(module: &AstModule, predicate: impl Fn(&AstExpr) -> bool) -> bool {
        module.items.iter().any(|item| match item {
            AstItem::Record(record) => record_contains_expr(record, &predicate),
            AstItem::Function(function) => expr_contains(&function.body, &predicate),
            AstItem::Expression(expr) => expr_contains(expr, &predicate),
        })
    }

    fn record_contains_expr(record: &AstRecord, predicate: &impl Fn(&AstExpr) -> bool) -> bool {
        record
            .value
            .as_ref()
            .is_some_and(|expr| expr_contains(expr, predicate))
            || record
                .children
                .iter()
                .any(|child| record_contains_expr(child, predicate))
    }

    fn expr_contains(expr: &AstExpr, predicate: &impl Fn(&AstExpr) -> bool) -> bool {
        if predicate(expr) {
            return true;
        }
        match &expr.kind {
            AstExprKind::Record { entries } => entries
                .iter()
                .any(|entry| record_contains_expr(entry, predicate)),
            AstExprKind::List { items } => items.iter().any(|item| expr_contains(item, predicate)),
            AstExprKind::Block { bindings } => bindings
                .iter()
                .any(|binding| expr_contains(&binding.value, predicate)),
            AstExprKind::When { arms } => {
                arms.iter().any(|arm| expr_contains(&arm.value, predicate))
            }
            AstExprKind::Then { body } | AstExprKind::Hold { body, .. } => {
                expr_contains(body, predicate)
            }
            AstExprKind::Latest { branches } => branches
                .iter()
                .any(|branch| expr_contains(branch, predicate)),
            AstExprKind::Call { args, .. } => {
                args.iter().any(|arg| expr_contains(&arg.value, predicate))
            }
            AstExprKind::Pipeline { input, stages } => {
                expr_contains(input, predicate)
                    || stages.iter().any(|stage| expr_contains(stage, predicate))
            }
            AstExprKind::Binary { left, right, .. } => {
                expr_contains(left, predicate) || expr_contains(right, predicate)
            }
            AstExprKind::Source
            | AstExprKind::Number { .. }
            | AstExprKind::Bool { .. }
            | AstExprKind::Tag { .. }
            | AstExprKind::Text { .. }
            | AstExprKind::Path { .. }
            | AstExprKind::Passed
            | AstExprKind::Skip
            | AstExprKind::Raw => false,
        }
    }
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
