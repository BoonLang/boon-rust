use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpressionBook {
    rows: usize,
    columns: usize,
    selected: (usize, usize),
    edit_focus: Option<(usize, usize)>,
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
            selected: (1, 1),
            edit_focus: None,
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

    pub fn selected(&self) -> (usize, usize) {
        self.selected
    }

    pub fn set_selected(&mut self, row: usize, col: usize) {
        self.selected = (row.clamp(1, self.rows), col.clamp(1, self.columns));
    }

    pub fn move_selected(&mut self, row_delta: isize, col_delta: isize) {
        let row = self
            .selected
            .0
            .saturating_add_signed(row_delta)
            .clamp(1, self.rows);
        let col = self
            .selected
            .1
            .saturating_add_signed(col_delta)
            .clamp(1, self.columns);
        self.selected = (row, col);
    }

    pub fn edit_focus(&self) -> Option<(usize, usize)> {
        self.edit_focus
    }

    pub fn set_edit_focus(&mut self, edit_focus: Option<(usize, usize)>) {
        self.edit_focus = edit_focus;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MotionAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MotionBody {
    pub x: i64,
    pub y: i64,
    pub dx: i64,
    pub dy: i64,
    pub size: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MotionControl {
    pub axis: MotionAxis,
    pub position: i64,
    pub step: i64,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub auto_track: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContactGrid {
    pub rows: usize,
    pub columns: usize,
    pub top: i64,
    pub margin: i64,
    pub gap: i64,
    pub height: i64,
    pub value_per_contact: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MotionConfig {
    pub arena_width: i64,
    pub arena_height: i64,
    pub body: MotionBody,
    pub primary_control: MotionControl,
    pub tracked_control: Option<MotionControl>,
    pub contact_grid: Option<ContactGrid>,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self {
            arena_width: 0,
            arena_height: 0,
            body: MotionBody {
                x: 0,
                y: 0,
                dx: 0,
                dy: 0,
                size: 0,
            },
            primary_control: MotionControl {
                axis: MotionAxis::Vertical,
                position: 50,
                step: 0,
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                auto_track: false,
            },
            tracked_control: None,
            contact_grid: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MotionDocument {
    config: MotionConfig,
    frame_index: u64,
    body_x: i64,
    body_y: i64,
    body_dx: i64,
    body_dy: i64,
    control_x: i64,
    control_y: i64,
    tracked_control_y: i64,
    contacts: Vec<bool>,
    contact_value: i64,
    resets_remaining: i64,
}

impl MotionDocument {
    pub fn new(config: MotionConfig) -> Self {
        let contact_rows = config.contact_grid.as_ref().map_or(0, |grid| grid.rows);
        let contact_columns = config.contact_grid.as_ref().map_or(0, |grid| grid.columns);
        Self {
            body_x: config.body.x,
            body_y: config.body.y,
            body_dx: config.body.dx,
            body_dy: config.body.dy,
            control_x: if matches!(config.primary_control.axis, MotionAxis::Horizontal) {
                config.primary_control.position
            } else {
                50
            },
            control_y: if matches!(config.primary_control.axis, MotionAxis::Vertical) {
                config.primary_control.position
            } else {
                50
            },
            tracked_control_y: config
                .tracked_control
                .as_ref()
                .map_or(50, |control| control.position),
            contacts: vec![true; contact_rows * contact_columns],
            contact_value: 0,
            resets_remaining: 3,
            frame_index: 0,
            config,
        }
    }

    pub fn empty() -> Self {
        Self::new(MotionConfig::default())
    }

    pub fn is_enabled(&self) -> bool {
        self.config.arena_width > 0 && self.config.arena_height > 0 && self.config.body.size > 0
    }

    pub fn config(&self) -> &MotionConfig {
        &self.config
    }

    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    pub fn body_x(&self) -> i64 {
        self.body_x
    }

    pub fn body_y(&self) -> i64 {
        self.body_y
    }

    pub fn body_dx(&self) -> i64 {
        self.body_dx
    }

    pub fn body_dy(&self) -> i64 {
        self.body_dy
    }

    pub fn control_x(&self) -> i64 {
        self.control_x
    }

    pub fn control_y(&self) -> i64 {
        self.control_y
    }

    pub fn tracked_control_y(&self) -> i64 {
        self.tracked_control_y
    }

    pub fn contact_rows(&self) -> usize {
        self.config
            .contact_grid
            .as_ref()
            .map_or(0, |grid| grid.rows)
    }

    pub fn contact_columns(&self) -> usize {
        self.config
            .contact_grid
            .as_ref()
            .map_or(0, |grid| grid.columns)
    }

    pub fn live_contact_count(&self) -> usize {
        self.contacts.iter().filter(|live| **live).count()
    }

    pub fn live_contact_indices(&self) -> String {
        self.contacts
            .iter()
            .enumerate()
            .filter_map(|(idx, live)| live.then_some(idx.to_string()))
            .collect::<Vec<_>>()
            .join(",")
    }

    pub fn contact_value(&self) -> i64 {
        self.contact_value
    }

    pub fn resets_remaining(&self) -> i64 {
        self.resets_remaining
    }

    pub fn contact_is_live(&self, idx: usize) -> bool {
        self.contacts.get(idx).copied().unwrap_or(false)
    }

    pub fn apply_key(&mut self, key: &str) {
        match self.config.primary_control.axis {
            MotionAxis::Horizontal => match key {
                "ArrowLeft" | "ArrowUp" => {
                    self.control_x = (self.control_x - self.config.primary_control.step).max(0);
                }
                "ArrowRight" | "ArrowDown" => {
                    self.control_x = (self.control_x + self.config.primary_control.step).min(100);
                }
                _ => {}
            },
            MotionAxis::Vertical => match key {
                "ArrowUp" | "ArrowLeft" => {
                    self.control_y = (self.control_y - self.config.primary_control.step).max(0);
                }
                "ArrowDown" | "ArrowRight" => {
                    self.control_y = (self.control_y + self.config.primary_control.step).min(100);
                }
                _ => {}
            },
        }
    }

    pub fn advance_frame(&mut self) {
        self.frame_index += 1;
        if self.config.contact_grid.is_some() {
            self.advance_contact_grid();
        } else {
            self.advance_tracked_control();
        }
    }

    fn advance_tracked_control(&mut self) {
        let Some(tracked_control) = self.config.tracked_control.as_ref() else {
            return;
        };
        let arena_w = self.config.arena_width;
        let arena_h = self.config.arena_height;
        let body_size = self.config.body.size;
        let control_w = self.config.primary_control.width;
        let control_h = self.config.primary_control.height;
        let left_x = self.config.primary_control.x;
        let right_x = tracked_control.x;

        self.body_x += self.body_dx;
        self.body_y += self.body_dy;
        if self.body_y <= 0 {
            self.body_y = 0;
            self.body_dy = self.body_dy.abs();
        } else if self.body_y + body_size >= arena_h {
            self.body_y = arena_h - body_size;
            self.body_dy = -self.body_dy.abs();
        }

        self.tracked_control_y = position_from_control_top(
            self.body_y + body_size / 2 - control_h / 2,
            arena_h,
            control_h,
        );
        let left_y = control_top_from_position(self.control_y, arena_h, control_h);
        let right_y = control_top_from_position(self.tracked_control_y, arena_h, control_h);

        if self.body_dx < 0
            && self.body_x <= left_x + control_w
            && self.body_x + body_size >= left_x
            && ranges_overlap(
                self.body_y,
                self.body_y + body_size,
                left_y,
                left_y + control_h,
            )
        {
            self.body_x = left_x + control_w;
            self.body_dx = self.body_dx.abs();
            self.body_dy = (self.body_dy
                + ((self.body_y + body_size / 2) - (left_y + control_h / 2)) / 18)
                .clamp(-18, 18);
            self.contact_value += 1;
        }
        if self.body_dx > 0
            && self.body_x + body_size >= right_x
            && self.body_x <= right_x + control_w
            && ranges_overlap(
                self.body_y,
                self.body_y + body_size,
                right_y,
                right_y + control_h,
            )
        {
            self.body_x = right_x - body_size;
            self.body_dx = -self.body_dx.abs();
            self.body_dy = (self.body_dy
                + ((self.body_y + body_size / 2) - (right_y + control_h / 2)) / 18)
                .clamp(-18, 18);
            self.contact_value += 1;
        }
        if self.body_x < -body_size || self.body_x > arena_w + body_size {
            self.body_x = arena_w / 2;
            self.body_y = arena_h / 2;
            self.body_dx = if self.body_dx < 0 { 12 } else { -12 };
            self.body_dy = 8;
            self.resets_remaining = (self.resets_remaining - 1).max(0);
        }
    }

    fn advance_contact_grid(&mut self) {
        let Some(contact_grid) = self.config.contact_grid.as_ref() else {
            return;
        };
        let arena_w = self.config.arena_width;
        let arena_h = self.config.arena_height;
        let body_size = self.config.body.size;
        let control_w = self.config.primary_control.width;
        let control_h = self.config.primary_control.height;
        let control_y = self.config.primary_control.y;

        self.body_x += self.body_dx;
        self.body_y += self.body_dy;
        if self.body_x <= 0 {
            self.body_x = 0;
            self.body_dx = self.body_dx.abs();
        } else if self.body_x + body_size >= arena_w {
            self.body_x = arena_w - body_size;
            self.body_dx = -self.body_dx.abs();
        }
        if self.body_y <= 0 {
            self.body_y = 0;
            self.body_dy = self.body_dy.abs();
        }

        if self.body_dy < 0 {
            let margin = contact_grid.margin;
            let gap = contact_grid.gap;
            let contact_h = contact_grid.height;
            let rows = self.contact_rows() as i64;
            let cols = self.contact_columns() as i64;
            let contact_w = if cols > 0 {
                (arena_w - margin * 2 - gap * (cols - 1)) / cols
            } else {
                0
            };
            'contact_scan: for row in 0..rows {
                for col in 0..cols {
                    let idx = (row * cols + col) as usize;
                    if !self.contacts.get(idx).copied().unwrap_or(false) {
                        continue;
                    }
                    let bx = margin + col * (contact_w + gap);
                    let by = contact_grid.top + row * (contact_h + gap);
                    if rects_overlap(
                        self.body_x,
                        self.body_y,
                        body_size,
                        body_size,
                        bx,
                        by,
                        contact_w,
                        contact_h,
                    ) {
                        self.contacts[idx] = false;
                        self.body_dy = self.body_dy.abs();
                        self.contact_value += contact_grid.value_per_contact;
                        break 'contact_scan;
                    }
                }
            }
        }

        let control_x = control_left_from_position(self.control_x, arena_w, control_w);
        if self.body_dy > 0
            && rects_overlap(
                self.body_x,
                self.body_y,
                body_size,
                body_size,
                control_x,
                control_y,
                control_w,
                control_h,
            )
        {
            self.body_y = control_y - body_size;
            self.body_dy = -self.body_dy.abs();
            self.body_dx = (self.body_dx
                + ((self.body_x + body_size / 2) - (control_x + control_w / 2)) / 18)
                .clamp(-18, 18);
        }
        if self.body_y > arena_h {
            self.body_x = control_x + control_w / 2 - body_size / 2;
            self.body_y = control_y - body_size - 2;
            self.body_dx = self.config.body.dx;
            self.body_dy = self.config.body.dy;
            self.resets_remaining = (self.resets_remaining - 1).max(0);
        }
        if self.contacts.iter().all(|live| !*live) {
            self.contacts.fill(true);
        }
    }
}

pub fn control_top_from_position(position: i64, arena_h: i64, control_h: i64) -> i64 {
    ((arena_h - control_h).max(0) * position.clamp(0, 100) / 100).clamp(0, arena_h - control_h)
}

pub fn control_left_from_position(position: i64, arena_w: i64, control_w: i64) -> i64 {
    ((arena_w - control_w).max(0) * position.clamp(0, 100) / 100).clamp(0, arena_w - control_w)
}

fn position_from_control_top(top: i64, arena_h: i64, control_h: i64) -> i64 {
    if arena_h <= control_h {
        0
    } else {
        (top.clamp(0, arena_h - control_h) * 100 / (arena_h - control_h)).clamp(0, 100)
    }
}

fn ranges_overlap(a_start: i64, a_end: i64, b_start: i64, b_end: i64) -> bool {
    a_start < b_end && b_start < a_end
}

#[allow(clippy::too_many_arguments)]
fn rects_overlap(ax: i64, ay: i64, aw: i64, ah: i64, bx: i64, by: i64, bw: i64, bh: i64) -> bool {
    ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by
}
