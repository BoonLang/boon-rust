use std::collections::{BTreeMap, BTreeSet};

/// Reusable formula storage and dependency evaluation for Boon applications.
///
/// This is an explicit stdlib boundary: callers provide dimensions, enabled
/// formula functions, and cell text; the type does not know about maintained
/// examples or renderer layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpressionBook {
    rows: usize,
    columns: usize,
    text: Vec<String>,
    value: Vec<String>,
    deps: Vec<Vec<usize>>,
    rev_deps: Vec<Vec<usize>>,
    functions: BTreeSet<String>,
}

impl ExpressionBook {
    pub fn new(rows: usize, columns: usize, functions: impl IntoIterator<Item = String>) -> Self {
        let len = rows * columns;
        Self {
            rows,
            columns,
            text: vec![String::new(); len],
            value: vec![String::new(); len],
            deps: vec![Vec::new(); len],
            rev_deps: vec![Vec::new(); len],
            functions: functions.into_iter().collect(),
        }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    pub fn value(&self, row: usize, col: usize) -> &str {
        &self.value[self.idx(row, col)]
    }

    pub fn text(&self, row: usize, col: usize) -> &str {
        &self.text[self.idx(row, col)]
    }

    pub fn set_text(&mut self, row: usize, col: usize, text: String) {
        let idx = self.idx(row, col);
        for dep in self.deps[idx].drain(..) {
            self.rev_deps[dep].retain(|dependent| *dependent != idx);
        }
        let deps = self.collect_expression_refs(&text);
        for dep in &deps {
            if !self.rev_deps[*dep].contains(&idx) {
                self.rev_deps[*dep].push(idx);
            }
        }
        self.deps[idx] = deps;
        self.text[idx] = text;
        self.recalc_dirty(idx);
    }

    pub fn parse_owner(&self, owner_id: &str) -> Option<(usize, usize)> {
        self.decode_ref_tuple(owner_id)
    }

    fn idx(&self, row: usize, col: usize) -> usize {
        (row - 1) * self.columns + (col - 1)
    }

    fn recalc_dirty(&mut self, changed: usize) {
        let mut dirty = BTreeSet::new();
        self.collect_dependents(changed, &mut dirty);
        let mut memo = BTreeMap::new();
        for idx in dirty {
            let value = self.evaluate_slot(idx, &mut BTreeSet::new(), &mut memo);
            self.value[idx] = value;
        }
    }

    fn collect_dependents(&self, idx: usize, dirty: &mut BTreeSet<usize>) {
        if dirty.insert(idx) {
            for dependent in &self.rev_deps[idx] {
                self.collect_dependents(*dependent, dirty);
            }
        }
    }

    fn evaluate_slot(
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
        let text = &self.text[idx];
        let value = if let Some(expression) = text.strip_prefix('=') {
            self.resolve_expression(expression, visiting, memo)
        } else {
            text.clone()
        };
        visiting.remove(&idx);
        memo.insert(idx, value.clone());
        value
    }

    fn resolve_expression(
        &self,
        expression: &str,
        visiting: &mut BTreeSet<usize>,
        memo: &mut BTreeMap<usize, String>,
    ) -> String {
        if self.functions.contains("add")
            && let Some(args) = expression
                .strip_prefix("add(")
                .and_then(|rest| rest.strip_suffix(')'))
        {
            let parts = args.split(',').map(str::trim).collect::<Vec<_>>();
            if parts.len() != 2 {
                return "#ERR".to_string();
            }
            let Some(left) = self.decode_ref(parts[0]) else {
                return "#ERR".to_string();
            };
            let Some(right) = self.decode_ref(parts[1]) else {
                return "#ERR".to_string();
            };
            let Some(left) = self.evaluate_number(left, visiting, memo) else {
                return "#CYCLE".to_string();
            };
            let Some(right) = self.evaluate_number(right, visiting, memo) else {
                return "#CYCLE".to_string();
            };
            return (left + right).to_string();
        }
        if self.functions.contains("sum")
            && let Some(args) = expression
                .strip_prefix("sum(")
                .and_then(|rest| rest.strip_suffix(')'))
        {
            let Some((start, end)) = self.parse_range(args.trim()) else {
                return "#ERR".to_string();
            };
            let mut sum = 0;
            for row in start.0.min(end.0)..=start.0.max(end.0) {
                for col in start.1.min(end.1)..=start.1.max(end.1) {
                    let Some(value) = self.evaluate_number(self.idx(row, col), visiting, memo)
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

    fn collect_expression_refs(&self, text: &str) -> Vec<usize> {
        let Some(expression) = text.strip_prefix('=') else {
            return Vec::new();
        };
        if self.functions.contains("add")
            && let Some(args) = expression
                .strip_prefix("add(")
                .and_then(|rest| rest.strip_suffix(')'))
        {
            return args
                .split(',')
                .filter_map(|arg| self.decode_ref(arg.trim()))
                .collect();
        }
        if self.functions.contains("sum")
            && let Some(args) = expression
                .strip_prefix("sum(")
                .and_then(|rest| rest.strip_suffix(')'))
            && let Some((start, end)) = self.parse_range(args.trim())
        {
            let mut deps = Vec::new();
            for row in start.0.min(end.0)..=start.0.max(end.0) {
                for col in start.1.min(end.1)..=start.1.max(end.1) {
                    deps.push(self.idx(row, col));
                }
            }
            return deps;
        }
        Vec::new()
    }

    fn parse_range(&self, text: &str) -> Option<((usize, usize), (usize, usize))> {
        let (start, end) = text.split_once(':')?;
        Some((self.decode_ref_tuple(start)?, self.decode_ref_tuple(end)?))
    }

    fn decode_ref(&self, text: &str) -> Option<usize> {
        let (row, col) = self.decode_ref_tuple(text)?;
        Some(self.idx(row, col))
    }

    fn decode_ref_tuple(&self, text: &str) -> Option<(usize, usize)> {
        let mut chars = text.chars();
        let col = chars.next()?.to_ascii_uppercase();
        if !col.is_ascii_uppercase() {
            return None;
        }
        let row = chars.as_str().parse::<usize>().ok()?;
        let col = (col as u8).checked_sub(b'A')? as usize + 1;
        if row == 0 || row > self.rows || col == 0 || col > self.columns {
            return None;
        }
        Some((row, col))
    }

    fn evaluate_number(
        &self,
        idx: usize,
        visiting: &mut BTreeSet<usize>,
        memo: &mut BTreeMap<usize, String>,
    ) -> Option<i64> {
        let value = self.evaluate_slot(idx, visiting, memo);
        if value == "#CYCLE" {
            None
        } else {
            Some(value.parse::<i64>().unwrap_or(0))
        }
    }
}

pub fn eval_number_call<F>(path: &str, mut arg: F) -> Result<i64, String>
where
    F: FnMut(&str) -> Result<i64, String>,
{
    match path {
        "Number/min" => Ok(arg("left")?.min(arg("right")?)),
        "Number/max" => Ok(arg("left")?.max(arg("right")?)),
        "Number/clamp" => Ok(arg("value")?.clamp(arg("min")?, arg("max")?)),
        "Number/abs" => Ok(arg("value")?.abs()),
        "Number/neg_abs" => Ok(-arg("value")?.abs()),
        "Number/scale_percent" => Ok(scale_percent(arg("value")?, arg("min")?, arg("max")?)),
        "Number/percent_of_range" => Ok(percent_of_range(arg("value")?, arg("min")?, arg("max")?)),
        _ => Err(format!("unsupported executable stdlib call `{path}`")),
    }
}

pub fn eval_bool_call<F>(path: &str, mut arg: F) -> Result<bool, String>
where
    F: FnMut(&str) -> Result<i64, String>,
{
    match path {
        "Number/less_than" => Ok(arg("left")? < arg("right")?),
        "Number/less_or_equal" => Ok(arg("left")? <= arg("right")?),
        "Number/greater_than" => Ok(arg("left")? > arg("right")?),
        "Number/greater_or_equal" => Ok(arg("left")? >= arg("right")?),
        "Geometry/intersects" => Ok(rectangles_intersect(
            arg("ax")?,
            arg("ay")?,
            arg("aw")?,
            arg("ah")?,
            arg("bx")?,
            arg("by")?,
            arg("bw")?,
            arg("bh")?,
        )),
        _ => Err(format!("unsupported executable stdlib predicate `{path}`")),
    }
}

fn scale_percent(value: i64, min: i64, max: i64) -> i64 {
    let low = min.min(max);
    let high = min.max(max);
    low + ((high - low) * value.clamp(0, 100) / 100)
}

fn percent_of_range(value: i64, min: i64, max: i64) -> i64 {
    let low = min.min(max);
    let high = min.max(max);
    if high == low {
        0
    } else {
        ((value.clamp(low, high) - low) * 100 / (high - low)).clamp(0, 100)
    }
}

#[allow(clippy::too_many_arguments)]
fn rectangles_intersect(
    ax: i64,
    ay: i64,
    aw: i64,
    ah: i64,
    bx: i64,
    by: i64,
    bw: i64,
    bh: i64,
) -> bool {
    ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by
}
