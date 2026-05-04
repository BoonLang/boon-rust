use std::collections::{BTreeMap, BTreeSet};

/// Reusable formula storage and dependency evaluation for Boon applications.
///
/// This is an explicit stdlib boundary: callers provide dimensions, enabled
/// formula functions, and cell text; the type does not know about maintained
/// examples or renderer layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormulaBook {
    rows: usize,
    columns: usize,
    text: Vec<String>,
    value: Vec<String>,
    deps: Vec<Vec<usize>>,
    rev_deps: Vec<Vec<usize>>,
    functions: BTreeSet<String>,
}

impl FormulaBook {
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
        "Geometry/track_vertical_position" => Ok(track_vertical_position(
            arg("body_y")?,
            arg("body_size")?,
            arg("arena_height")?,
            arg("height")?,
        )),
        "Geometry/peer_body_x" => peer_body_next(&mut arg, PeerQuantity::X),
        "Geometry/peer_body_y" => peer_body_next(&mut arg, PeerQuantity::Y),
        "Geometry/peer_body_dx" => peer_body_next(&mut arg, PeerQuantity::Dx),
        "Geometry/peer_body_dy" => peer_body_next(&mut arg, PeerQuantity::Dy),
        "Geometry/peer_contact_value" => peer_body_next(&mut arg, PeerQuantity::ContactValue),
        "Geometry/peer_resets_remaining" => peer_body_next(&mut arg, PeerQuantity::ResetsRemaining),
        "Geometry/contact_body_x" => contact_body_next(&mut arg, ContactQuantity::X),
        "Geometry/contact_body_y" => contact_body_next(&mut arg, ContactQuantity::Y),
        "Geometry/contact_body_dx" => contact_body_next(&mut arg, ContactQuantity::Dx),
        "Geometry/contact_body_dy" => contact_body_next(&mut arg, ContactQuantity::Dy),
        "Geometry/contact_live_count" => contact_body_next(&mut arg, ContactQuantity::LiveCount),
        "Geometry/contact_value" => contact_body_next(&mut arg, ContactQuantity::ContactValue),
        "Geometry/contact_resets_remaining" => {
            contact_body_next(&mut arg, ContactQuantity::ResetsRemaining)
        }
        _ => Err(format!("unsupported executable stdlib call `{path}`")),
    }
}

fn controller_top_from_position(position: i64, arena_h: i64, controller_h: i64) -> i64 {
    ((arena_h - controller_h).max(0) * position.clamp(0, 100) / 100)
        .clamp(0, arena_h - controller_h)
}

fn controller_left_from_position(position: i64, arena_w: i64, controller_w: i64) -> i64 {
    ((arena_w - controller_w).max(0) * position.clamp(0, 100) / 100)
        .clamp(0, arena_w - controller_w)
}

fn track_vertical_position(body_y: i64, body_size: i64, arena_h: i64, control_h: i64) -> i64 {
    if arena_h <= control_h {
        0
    } else {
        let top = body_y + body_size / 2 - control_h / 2;
        (top.clamp(0, arena_h - control_h) * 100 / (arena_h - control_h)).clamp(0, 100)
    }
}

#[derive(Clone, Copy)]
enum PeerQuantity {
    X,
    Y,
    Dx,
    Dy,
    ContactValue,
    ResetsRemaining,
}

fn peer_body_next<F>(arg: &mut F, quantity: PeerQuantity) -> Result<i64, String>
where
    F: FnMut(&str) -> Result<i64, String>,
{
    let arena_w = arg("arena_width")?;
    let arena_h = arg("arena_height")?;
    let body_size = arg("body_size")?;
    let control_w = arg("control_width")?;
    let control_h = arg("control_height")?;
    let left_x = arg("left_x")?;
    let right_x = arg("right_x")?;
    let control_y = arg("control_y")?;
    let tracked_control_y = arg("tracked_control_y")?;
    let mut x = arg("x")? + arg("dx")?;
    let mut y = arg("y")? + arg("dy")?;
    let mut dx = arg("dx")?;
    let mut dy = arg("dy")?;
    let mut contact_value = arg("contact_value")?;
    let mut resets_remaining = arg("resets_remaining")?;

    if y <= 0 {
        y = 0;
        dy = dy.abs();
    } else if y + body_size >= arena_h {
        y = arena_h - body_size;
        dy = -dy.abs();
    }

    let left_y = controller_top_from_position(control_y, arena_h, control_h);
    let right_y = controller_top_from_position(tracked_control_y, arena_h, control_h);
    if dx < 0
        && x <= left_x + control_w
        && x + body_size >= left_x
        && ranges_intersect(y, y + body_size, left_y, left_y + control_h)
    {
        x = left_x + control_w;
        dx = dx.abs();
        dy = (dy + ((y + body_size / 2) - (left_y + control_h / 2)) / 18).clamp(-18, 18);
        contact_value += 1;
    }
    if dx > 0
        && x + body_size >= right_x
        && x <= right_x + control_w
        && ranges_intersect(y, y + body_size, right_y, right_y + control_h)
    {
        x = right_x - body_size;
        dx = -dx.abs();
        dy = (dy + ((y + body_size / 2) - (right_y + control_h / 2)) / 18).clamp(-18, 18);
        contact_value += 1;
    }
    if x < -body_size || x > arena_w + body_size {
        x = arena_w / 2;
        y = arena_h / 2;
        dx = if dx < 0 { 12 } else { -12 };
        dy = 8;
        resets_remaining = (resets_remaining - 1).max(0);
    }

    Ok(match quantity {
        PeerQuantity::X => x,
        PeerQuantity::Y => y,
        PeerQuantity::Dx => dx,
        PeerQuantity::Dy => dy,
        PeerQuantity::ContactValue => contact_value,
        PeerQuantity::ResetsRemaining => resets_remaining,
    })
}

#[derive(Clone, Copy)]
enum ContactQuantity {
    X,
    Y,
    Dx,
    Dy,
    LiveCount,
    ContactValue,
    ResetsRemaining,
}

fn contact_body_next<F>(arg: &mut F, quantity: ContactQuantity) -> Result<i64, String>
where
    F: FnMut(&str) -> Result<i64, String>,
{
    let arena_w = arg("arena_width")?;
    let arena_h = arg("arena_height")?;
    let body_size = arg("body_size")?;
    let control_w = arg("control_width")?;
    let control_h = arg("control_height")?;
    let control_y = arg("control_y")?;
    let margin = arg("field_margin")?;
    let top = arg("field_top")?;
    let gap = arg("field_gap")?;
    let cell_h = arg("field_height")?;
    let rows = arg("field_rows")?.max(0);
    let columns = arg("field_columns")?.max(1);
    let value_per_contact = arg("value_per_contact")?;
    let mut x = arg("x")? + arg("dx")?;
    let mut y = arg("y")? + arg("dy")?;
    let mut dx = arg("dx")?;
    let mut dy = arg("dy")?;
    let mut live_count = arg("live_count")?.clamp(0, rows * columns);
    let mut contact_value = arg("contact_value")?;
    let mut resets_remaining = arg("resets_remaining")?;

    if x <= 0 {
        x = 0;
        dx = dx.abs();
    } else if x + body_size >= arena_w {
        x = arena_w - body_size;
        dx = -dx.abs();
    }
    if y <= 0 {
        y = 0;
        dy = dy.abs();
    }

    if dy < 0 && live_count > 0 {
        let cell_w = (arena_w - margin * 2 - gap * (columns - 1)) / columns;
        let mut hit = false;
        'scan: for row in 0..rows {
            for column in 0..columns {
                let index = row * columns + column;
                if index >= live_count {
                    continue;
                }
                let bx = margin + column * (cell_w + gap);
                let by = top + row * (cell_h + gap);
                if rectangles_intersect(x, y, body_size, body_size, bx, by, cell_w, cell_h) {
                    hit = true;
                    break 'scan;
                }
            }
        }
        if hit {
            live_count -= 1;
            dy = dy.abs();
            contact_value += value_per_contact;
        }
    }

    let control_x = controller_left_from_position(arg("control_x")?, arena_w, control_w);
    if dy > 0
        && rectangles_intersect(
            x, y, body_size, body_size, control_x, control_y, control_w, control_h,
        )
    {
        y = control_y - body_size;
        dy = -dy.abs();
        dx = (dx + ((x + body_size / 2) - (control_x + control_w / 2)) / 18).clamp(-18, 18);
    }
    if y > arena_h {
        x = control_x + control_w / 2 - body_size / 2;
        y = control_y - body_size - 2;
        dx = arg("initial_dx")?;
        dy = arg("initial_dy")?;
        resets_remaining = (resets_remaining - 1).max(0);
    }
    if live_count == 0 {
        live_count = rows * columns;
    }

    Ok(match quantity {
        ContactQuantity::X => x,
        ContactQuantity::Y => y,
        ContactQuantity::Dx => dx,
        ContactQuantity::Dy => dy,
        ContactQuantity::LiveCount => live_count,
        ContactQuantity::ContactValue => contact_value,
        ContactQuantity::ResetsRemaining => resets_remaining,
    })
}

fn ranges_intersect(a_start: i64, a_end: i64, b_start: i64, b_end: i64) -> bool {
    a_start < b_end && b_start < a_end
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
