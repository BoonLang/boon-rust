#[derive(Clone, Debug)]
pub struct ExampleApp {
    program: ProgramSpec,
    inventory: SourceInventory,
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
    grid: GridModel,
    game_frame: u64,
    game: GameModel,
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
struct GridModel {
    rows: usize,
    columns: usize,
    selected: (usize, usize),
    editing: Option<(usize, usize)>,
    text: Vec<String>,
    value: Vec<String>,
    deps: Vec<Vec<usize>>,
    rev_deps: Vec<Vec<usize>>,
}

impl GridModel {
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
struct GameModel {
    ball_x: i64,
    ball_y: i64,
    ball_dx: i64,
    ball_dy: i64,
    paddle_x: i64,
    paddle_y: i64,
    right_paddle_y: i64,
    bricks_rows: usize,
    bricks_cols: usize,
    bricks: Vec<bool>,
    score: i64,
    lives: i64,
}

impl GameModel {
    fn pong() -> Self {
        Self {
            ball_x: 84,
            ball_y: 350,
            ball_dx: -12,
            ball_dy: 8,
            paddle_x: 50,
            paddle_y: 50,
            right_paddle_y: 50,
            bricks_rows: 0,
            bricks_cols: 0,
            bricks: Vec::new(),
            score: 0,
            lives: 3,
        }
    }

    fn arkanoid() -> Self {
        let rows = 6;
        let cols = 12;
        Self {
            ball_x: 470,
            ball_y: 205,
            ball_dx: 10,
            ball_dy: -12,
            paddle_x: 50,
            paddle_y: 50,
            right_paddle_y: 50,
            bricks_rows: rows,
            bricks_cols: cols,
            bricks: vec![true; rows * cols],
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

impl ExampleApp {
    fn new(compiled: boon_compiler::CompiledModule) -> Self {
        let inventory = compiled.sources;
        let program = compiled.program;
        let initial_titles = program
            .keyed_list
            .as_ref()
            .map(|list_item| list_item.initial_titles.clone())
            .unwrap_or_default();
        let grid = program
            .grid
            .as_ref()
            .map(|grid| GridModel::new(grid.rows, grid.columns))
            .unwrap_or_else(|| GridModel::new(100, 26));
        let is_arkanoid = program.title.contains("Arkanoid");
        let mut app = Self {
            program,
            inventory,
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
            grid,
            game_frame: 0,
            game: if is_arkanoid {
                GameModel::arkanoid()
            } else {
                GameModel::pong()
            },
        };
        app.next_list_item_id = app.list_items.len() as u64 + 1;
        app.frame_text = app.render_text();
        app
    }

    fn emit_frame(&mut self, changed: &[&str], mut metrics: TurnMetrics) -> TurnResult {
        self.turn += 1;
        self.frame_text = self.render_text();
        metrics.patch_count = 1;
        TurnResult {
            turn_id: TurnId(self.turn),
            patches: vec![HostPatch::ReplaceFrameText {
                text: self.frame_text.clone(),
            }],
            state_delta: StateDelta {
                changed_paths: changed.iter().map(|s| (*s).to_string()).collect(),
            },
            metrics,
        }
    }

    fn render_text(&self) -> String {
        if self.program.keyed_list.is_some() {
            self.render_keyed_list()
        } else if self.program.grid.is_some() {
            self.render_grid()
        } else if self.program.counter {
            format!("Boon Counter\n[ Increment ]\ncount: {}", self.counter)
        } else if self.program.interval {
            format!(
                "Boon Interval\nfake_clock_ms: {}\nticks: {}",
                self.clock.millis, self.interval_count
            )
        } else if self.program.frame_counter {
            self.render_frame_counter()
        } else {
            String::new()
        }
    }

    fn render_keyed_list(&self) -> String {
        let completed = self.list_items.iter().filter(|list_item| list_item.completed).count();
        let active = self.list_items.len().saturating_sub(completed);
        let mut lines = vec![
            self.program.title.clone(),
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

    fn render_frame_counter(&self) -> String {
        format!(
            "{}\nframe: {}\npaddle_y: {}\npaddle_x: {}\nright_paddle_y: {}\nball_x: {}\nball_y: {}\nball_dx: {}\nball_dy: {}\nbricks_rows: {}\nbricks_cols: {}\nbricks_live: {}\nscore: {}\nlives: {}\ndeterministic input source: store.sources.tick.event.frame",
            self.program.title,
            self.game_frame,
            self.game.paddle_y,
            self.game.paddle_x,
            self.game.right_paddle_y,
            self.game.ball_x,
            self.game.ball_y,
            self.game.ball_dx,
            self.game.ball_dy,
            self.game.bricks_rows,
            self.game.bricks_cols,
            self.game.live_brick_indices(),
            self.game.score,
            self.game.lives
        )
    }

    fn advance_game_frame(&mut self) {
        self.game_frame += 1;
        if self.program.title.contains("Arkanoid") {
            self.advance_arkanoid_frame();
        } else {
            self.advance_pong_frame();
        }
    }

    fn advance_pong_frame(&mut self) {
        const ARENA_W: i64 = 1000;
        const ARENA_H: i64 = 700;
        const BALL: i64 = 22;
        const PADDLE_W: i64 = 18;
        const PADDLE_H: i64 = 128;
        const LEFT_X: i64 = 38;
        const RIGHT_X: i64 = ARENA_W - LEFT_X - PADDLE_W;

        self.game.ball_x += self.game.ball_dx;
        self.game.ball_y += self.game.ball_dy;
        if self.game.ball_y <= 0 {
            self.game.ball_y = 0;
            self.game.ball_dy = self.game.ball_dy.abs();
        } else if self.game.ball_y + BALL >= ARENA_H {
            self.game.ball_y = ARENA_H - BALL;
            self.game.ball_dy = -self.game.ball_dy.abs();
        }

        self.game.right_paddle_y =
            position_from_paddle_top(self.game.ball_y + BALL / 2 - PADDLE_H / 2, ARENA_H, PADDLE_H);
        let left_y = paddle_top_from_position(self.game.paddle_y, ARENA_H, PADDLE_H);
        let right_y = paddle_top_from_position(self.game.right_paddle_y, ARENA_H, PADDLE_H);

        if self.game.ball_dx < 0
            && self.game.ball_x <= LEFT_X + PADDLE_W
            && self.game.ball_x + BALL >= LEFT_X
            && ranges_overlap(self.game.ball_y, self.game.ball_y + BALL, left_y, left_y + PADDLE_H)
        {
            self.game.ball_x = LEFT_X + PADDLE_W;
            self.game.ball_dx = self.game.ball_dx.abs();
            self.game.ball_dy = (self.game.ball_dy
                + ((self.game.ball_y + BALL / 2) - (left_y + PADDLE_H / 2)) / 18)
                .clamp(-18, 18);
            self.game.score += 1;
        }
        if self.game.ball_dx > 0
            && self.game.ball_x + BALL >= RIGHT_X
            && self.game.ball_x <= RIGHT_X + PADDLE_W
            && ranges_overlap(
                self.game.ball_y,
                self.game.ball_y + BALL,
                right_y,
                right_y + PADDLE_H,
            )
        {
            self.game.ball_x = RIGHT_X - BALL;
            self.game.ball_dx = -self.game.ball_dx.abs();
            self.game.ball_dy = (self.game.ball_dy
                + ((self.game.ball_y + BALL / 2) - (right_y + PADDLE_H / 2)) / 18)
                .clamp(-18, 18);
            self.game.score += 1;
        }
        if self.game.ball_x < -BALL || self.game.ball_x > ARENA_W + BALL {
            self.game.ball_x = ARENA_W / 2;
            self.game.ball_y = ARENA_H / 2;
            self.game.ball_dx = if self.game.ball_dx < 0 { 12 } else { -12 };
            self.game.ball_dy = 8;
            self.game.lives = (self.game.lives - 1).max(0);
        }
    }

    fn advance_arkanoid_frame(&mut self) {
        const ARENA_W: i64 = 1000;
        const ARENA_H: i64 = 700;
        const BALL: i64 = 22;
        const PADDLE_W: i64 = 160;
        const PADDLE_H: i64 = 18;
        const PADDLE_Y: i64 = 646;

        self.game.ball_x += self.game.ball_dx;
        self.game.ball_y += self.game.ball_dy;
        if self.game.ball_x <= 0 {
            self.game.ball_x = 0;
            self.game.ball_dx = self.game.ball_dx.abs();
        } else if self.game.ball_x + BALL >= ARENA_W {
            self.game.ball_x = ARENA_W - BALL;
            self.game.ball_dx = -self.game.ball_dx.abs();
        }
        if self.game.ball_y <= 0 {
            self.game.ball_y = 0;
            self.game.ball_dy = self.game.ball_dy.abs();
        }

        if self.game.ball_dy < 0 {
            let margin = 36;
            let gap = 8;
            let brick_h = 28;
            let rows = self.game.bricks_rows as i64;
            let cols = self.game.bricks_cols as i64;
            let brick_w = if cols > 0 {
                (ARENA_W - margin * 2 - gap * (cols - 1)) / cols
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
                    let by = 56 + row * (brick_h + gap);
                    if rects_overlap(
                        self.game.ball_x,
                        self.game.ball_y,
                        BALL,
                        BALL,
                        bx,
                        by,
                        brick_w,
                        brick_h,
                    ) {
                        self.game.bricks[idx] = false;
                        self.game.ball_dy = self.game.ball_dy.abs();
                        self.game.score += 10;
                        break 'brick_scan;
                    }
                }
            }
        }

        let paddle_x = paddle_left_from_position(self.game.paddle_x, ARENA_W, PADDLE_W);
        if self.game.ball_dy > 0
            && rects_overlap(
                self.game.ball_x,
                self.game.ball_y,
                BALL,
                BALL,
                paddle_x,
                PADDLE_Y,
                PADDLE_W,
                PADDLE_H,
            )
        {
            self.game.ball_y = PADDLE_Y - BALL;
            self.game.ball_dy = -self.game.ball_dy.abs();
            self.game.ball_dx = (self.game.ball_dx
                + ((self.game.ball_x + BALL / 2) - (paddle_x + PADDLE_W / 2)) / 18)
                .clamp(-18, 18);
        }
        if self.game.ball_y > ARENA_H {
            self.game.ball_x = paddle_x + PADDLE_W / 2 - BALL / 2;
            self.game.ball_y = PADDLE_Y - BALL - 2;
            self.game.ball_dx = 10;
            self.game.ball_dy = -12;
            self.game.lives = (self.game.lives - 1).max(0);
        }
        if self.game.bricks.iter().all(|live| !*live) {
            self.game.bricks.fill(true);
        }
    }

    fn visible_keyed_items(&self) -> impl Iterator<Item = &ListItem> {
        self.list_items.iter().filter(|list_item| match self.filter.as_str() {
            "active" => !list_item.completed,
            "completed" => list_item.completed,
            _ => true,
        })
    }

    fn render_grid(&self) -> String {
        let mut lines = vec![
            self.program.title.clone(),
            format!(
                "selected: {}{}",
                column_name(self.grid.selected.1),
                self.grid.selected.0
            ),
            format!(
                "formula: {}",
                self.grid_text(self.grid.selected.0, self.grid.selected.1)
            ),
            format!(
                "value: {}",
                self.grid_value(self.grid.selected.0, self.grid.selected.1)
            ),
            "columns: A B C D E F ... Z".to_string(),
        ];
        for row in 1..=self.grid.rows.min(5) {
            lines.push(format!(
                "row {row}: A={} | B={} | C={}",
                self.grid_value(row, 1.min(self.grid.columns)),
                self.grid_value(row, 2.min(self.grid.columns)),
                self.grid_value(row, 3.min(self.grid.columns))
            ));
        }
        lines.push(format!(
            "row {} and column {} reachable",
            self.grid.rows,
            column_name(self.grid.columns)
        ));
        lines.join("\n")
    }

    fn grid_value(&self, row: usize, col: usize) -> &str {
        &self.grid.value[self.grid_idx(row, col)]
    }

    fn grid_text(&self, row: usize, col: usize) -> &str {
        &self.grid.text[self.grid_idx(row, col)]
    }

    fn grid_idx(&self, row: usize, col: usize) -> usize {
        (row - 1) * self.grid.columns + (col - 1)
    }

    fn set_grid_text(&mut self, row: usize, col: usize, text: String) {
        let idx = self.grid_idx(row, col);
        for dep in self.grid.deps[idx].drain(..) {
            self.grid.rev_deps[dep].retain(|dependent| *dependent != idx);
        }
        let deps = self.formula_dependencies(&text);
        for dep in &deps {
            if !self.grid.rev_deps[*dep].contains(&idx) {
                self.grid.rev_deps[*dep].push(idx);
            }
        }
        self.grid.deps[idx] = deps;
        self.grid.text[idx] = text;
        self.recalc_dirty_grid(idx);
    }

    fn recalc_dirty_grid(&mut self, changed: usize) {
        let mut dirty = BTreeSet::new();
        self.collect_grid_dependents(changed, &mut dirty);
        let mut memo = BTreeMap::new();
        for idx in dirty {
            let value = self.evaluate_cell(idx, &mut BTreeSet::new(), &mut memo);
            self.grid.value[idx] = value;
        }
    }

    fn collect_grid_dependents(&self, idx: usize, dirty: &mut BTreeSet<usize>) {
        if dirty.insert(idx) {
            for dependent in &self.grid.rev_deps[idx] {
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
        let text = &self.grid.text[idx];
        let value = if let Some(formula) = text.strip_prefix('=') {
            self.evaluate_formula(formula, visiting, memo)
        } else {
            text.clone()
        };
        visiting.remove(&idx);
        memo.insert(idx, value.clone());
        value
    }

    fn evaluate_formula(
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
            let Some(left) = self.parse_cell_ref(parts[0]) else {
                return "#ERR".to_string();
            };
            let Some(right) = self.parse_cell_ref(parts[1]) else {
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
        self.program.grid.as_ref().is_some_and(|grid| {
            grid
                .formula_functions
                .iter()
                .any(|function| function == name)
        })
    }

    fn formula_dependencies(&self, text: &str) -> Vec<usize> {
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
                .filter_map(|arg| self.parse_cell_ref(arg.trim()))
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
            self.parse_cell_ref_tuple(start)?,
            self.parse_cell_ref_tuple(end)?,
        ))
    }

    fn parse_cell_ref(&self, text: &str) -> Option<usize> {
        let (row, col) = self.parse_cell_ref_tuple(text)?;
        Some(self.grid_idx(row, col))
    }

    fn parse_cell_ref_tuple(&self, text: &str) -> Option<(usize, usize)> {
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

    fn parse_grid_owner(&self, owner_id: &str) -> Result<(usize, usize)> {
        self.parse_cell_ref_tuple(owner_id)
            .ok_or_else(|| anyhow::anyhow!("grid_cell owner_id `{owner_id}` is outside compiled grid"))
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
        if path.starts_with("todos[*].") {
            let list_item_id = owner_id
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("list_item owner_id `{owner_id}` is not numeric"))?;
            return self
                .list_items
                .iter()
                .find(|list_item| list_item.id == list_item_id)
                .map(|list_item| list_item.generation)
                .ok_or_else(|| anyhow::anyhow!("dynamic list_item owner `{owner_id}` is not live"));
        }
        if path.starts_with("cells[*].") {
            self.parse_grid_owner(owner_id)?;
            return Ok(0);
        }
        bail!("dynamic SOURCE `{path}` has no owner generation table")
    }
}

impl BoonApp for ExampleApp {
    fn mount(&mut self) -> TurnResult {
        let patches = vec![
            HostPatch::CreateNode {
                id: NodeId(0),
                kind: NodeKind::Root,
                parent: None,
                key: None,
            },
            HostPatch::ReplaceFrameText {
                text: self.frame_text.clone(),
            },
        ];
        TurnResult {
            turn_id: TurnId(0),
            patches,
            state_delta: StateDelta::default(),
            metrics: TurnMetrics {
                patch_count: 2,
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
            if update.path.ends_with("new_todo_input.text") {
                if let SourceValue::Text(value) = update.value {
                    self.input_text = value;
                }
            } else if update.path.contains("todos[*].sources.edit_input.text")
                && let SourceValue::Text(value) = update.value
            {
                let owner_id = update
                    .owner_id
                    .as_deref()
                    .expect("dynamic todo edit_input.text owner_id was validated");
                let list_item_id = owner_id
                    .parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("list_item owner_id `{owner_id}` is not numeric"))?;
                let list_item = self
                    .list_items
                    .iter_mut()
                    .find(|list_item| list_item.id == list_item_id)
                    .ok_or_else(|| anyhow::anyhow!("list_item owner `{owner_id}` is not live"))?;
                list_item.title = value;
                list_item.editing = true;
            } else if update.path.contains("cells")
                && update.path.ends_with("editor.text")
                && let SourceValue::Text(value) = update.value
            {
                let (row, col) = update
                    .owner_id
                    .as_deref()
                    .map(|owner_id| self.parse_grid_owner(owner_id))
                    .transpose()?
                    .unwrap_or(self.grid.selected);
                self.set_grid_text(row, col, value);
            }
        }

        let mut results = Vec::new();
        for event in batch.events {
            let mut metrics = TurnMetrics {
                events_processed: 1,
                ..TurnMetrics::default()
            };
            if event.path.contains("increment_button.event.press") {
                self.counter += 1;
                results.push(self.emit_frame(&["counter"], metrics));
            } else if event.path.ends_with("new_todo_input.event.key_down.key") {
                if matches!(event.value, SourceValue::Tag(ref key) if key == "Enter") {
                    let trimmed = self.input_text.trim().to_string();
                    if !trimmed.is_empty() {
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
                results.push(self.emit_frame(
                    &["store.todos", "store.sources.new_todo_input.text"],
                    metrics,
                ));
            } else if event.path.ends_with("new_todo_input.event.focus")
                || event.path.ends_with("new_todo_input.event.blur")
                || event.path.ends_with("new_todo_input.event.change")
            {
                results.push(self.emit_frame(&["store.sources.new_todo_input"], metrics));
            } else if event.path.contains("toggle_all_checkbox.event.click") {
                let all_completed = self.list_items.iter().all(|list_item| list_item.completed);
                for list_item in &mut self.list_items {
                    list_item.completed = !all_completed;
                }
                metrics.list_rows_touched = self.list_items.len();
                results.push(self.emit_frame(&["store.completed_todos_count"], metrics));
            } else if event.path.contains("todos[*].sources.checkbox.event.click") {
                let owner_id = event
                    .owner_id
                    .as_deref()
                    .expect("dynamic event owner_id was validated");
                let list_item_id = owner_id
                    .parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("list_item owner_id `{owner_id}` is not numeric"))?;
                let list_item = self
                    .list_items
                    .iter_mut()
                    .find(|list_item| list_item.id == list_item_id)
                    .ok_or_else(|| anyhow::anyhow!("list_item owner `{owner_id}` is not live"))?;
                list_item.completed = !list_item.completed;
                metrics.list_rows_touched = 1;
                results.push(self.emit_frame(&["store.completed_todos_count"], metrics));
            } else if event
                .path
                .contains("todos[*].sources.remove_button.event.press")
            {
                let owner_id = event
                    .owner_id
                    .as_deref()
                    .expect("dynamic event owner_id was validated");
                let list_item_id = owner_id
                    .parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("list_item owner_id `{owner_id}` is not numeric"))?;
                self.list_items.retain(|list_item| list_item.id != list_item_id);
                results.push(self.emit_frame(&["store.todos"], metrics));
            } else if event.path.contains("clear_completed_button.event.press") {
                self.list_items.retain(|list_item| !list_item.completed);
                results.push(self.emit_frame(&["store.todos"], metrics));
            } else if event.path.contains("filter_active") {
                self.filter = "active".to_string();
                results.push(self.emit_frame(&["store.selected_filter"], metrics));
            } else if event.path.contains("filter_completed") {
                self.filter = "completed".to_string();
                results.push(self.emit_frame(&["store.selected_filter"], metrics));
            } else if event.path.contains("filter_all") {
                self.filter = "all".to_string();
                results.push(self.emit_frame(&["store.selected_filter"], metrics));
            } else if event
                .path
                .contains("todos[*].sources.edit_input.event.key_down.key")
            {
                let owner_id = event
                    .owner_id
                    .as_deref()
                    .expect("dynamic edit_input key owner_id was validated");
                let list_item_id = owner_id
                    .parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("list_item owner_id `{owner_id}` is not numeric"))?;
                if let Some(list_item) = self
                    .list_items
                    .iter_mut()
                    .find(|list_item| list_item.id == list_item_id)
                    && matches!(event.value, SourceValue::Tag(ref key) if key == "Enter")
                {
                    list_item.editing = false;
                }
                results.push(self.emit_frame(&["store.todos"], metrics));
            } else if event
                .path
                .contains("todos[*].sources.edit_input.event.blur")
                || event
                    .path
                    .contains("todos[*].sources.edit_input.event.change")
            {
                let owner_id = event
                    .owner_id
                    .as_deref()
                    .expect("dynamic edit_input event owner_id was validated");
                let list_item_id = owner_id
                    .parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("list_item owner_id `{owner_id}` is not numeric"))?;
                if let Some(list_item) = self
                    .list_items
                    .iter_mut()
                    .find(|list_item| list_item.id == list_item_id)
                    && event.path.ends_with(".event.blur")
                {
                    list_item.editing = false;
                }
                results.push(self.emit_frame(&["store.todos"], metrics));
            } else if event
                .path
                .contains("cells[*].sources.display.event.double_click")
            {
                let (row, col) = event
                    .owner_id
                    .as_deref()
                    .map(|owner_id| self.parse_grid_owner(owner_id))
                    .transpose()?
                    .unwrap_or(self.grid.selected);
                self.grid.selected = (row, col);
                self.grid.editing = Some((row, col));
                results.push(self.emit_frame(&["cells"], metrics));
            } else if event.path.contains("cells")
                && event.path.ends_with("editor.event.key_down.key")
            {
                if matches!(event.value, SourceValue::Tag(ref key) if key == "Enter") {
                    self.grid.editing = None;
                }
                results.push(self.emit_frame(&["cells"], metrics));
            } else if event.path.ends_with("store.sources.viewport.event.key_down.key") {
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
                            self.grid.selected.1 = (self.grid.selected.1 + 1).min(self.grid.columns);
                        }
                        _ => {}
                    }
                }
                results.push(self.emit_frame(&["cells.selected"], metrics));
            } else if event.path.ends_with("store.sources.paddle.event.key_down.key") {
                if let SourceValue::Tag(key) = &event.value {
                    if self.program.title.contains("Arkanoid") {
                        match key.as_str() {
                            "ArrowLeft" | "ArrowUp" => {
                                self.game.paddle_x = (self.game.paddle_x - 8).max(0);
                            }
                            "ArrowRight" | "ArrowDown" => {
                                self.game.paddle_x = (self.game.paddle_x + 8).min(100);
                            }
                            _ => {}
                        }
                    } else {
                        match key.as_str() {
                            "ArrowUp" | "ArrowLeft" => {
                                self.game.paddle_y = (self.game.paddle_y - 8).max(0);
                            }
                            "ArrowDown" | "ArrowRight" => {
                                self.game.paddle_y = (self.game.paddle_y + 8).min(100);
                            }
                            _ => {}
                        }
                    }
                }
                results.push(self.emit_frame(&["game.paddle"], metrics));
            } else {
                self.advance_game_frame();
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
        let completed = self.list_items.iter().filter(|list_item| list_item.completed).count() as i64;
        let mut values = BTreeMap::new();
        values.insert("counter".to_string(), json!(self.counter));
        values.insert(
            "store.todos_count".to_string(),
            json!(self.list_items.len() as i64),
        );
        values.insert("store.completed_todos_count".to_string(), json!(completed));
        values.insert(
            "store.active_todos_count".to_string(),
            json!(self.list_items.len() as i64 - completed),
        );
        values.insert("interval_count".to_string(), json!(self.interval_count));
        values.insert(
            "store.sources.new_todo_input.text".to_string(),
            json!(self.input_text),
        );
        values.insert(
            "store.todos_titles".to_string(),
            json!(self
                .list_items
                .iter()
                .map(|list_item| list_item.title.clone())
                .collect::<Vec<_>>()),
        );
        values.insert(
            "store.todos_ids".to_string(),
            json!(self
                .list_items
                .iter()
                .map(|list_item| list_item.id)
                .collect::<Vec<_>>()),
        );
        values.insert(
            "store.visible_todos_ids".to_string(),
            json!(self
                .visible_keyed_items()
                .map(|list_item| list_item.id)
                .collect::<Vec<_>>()),
        );
        values.insert("store.selected_filter".to_string(), json!(self.filter));
        for list_item in &self.list_items {
            values.insert(
                format!("store.todos[{}].title", list_item.id),
                json!(list_item.title),
            );
            values.insert(
                format!("store.todos[{}].completed", list_item.id),
                json!(list_item.completed),
            );
            values.insert(
                format!("store.todos[{}].editing", list_item.id),
                json!(list_item.editing),
            );
        }
        values.insert("game.frame".to_string(), json!(self.game_frame));
        values.insert("game.paddle_y".to_string(), json!(self.game.paddle_y));
        values.insert("game.paddle_x".to_string(), json!(self.game.paddle_x));
        values.insert(
            "game.right_paddle_y".to_string(),
            json!(self.game.right_paddle_y),
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
            "game.bricks_live_count".to_string(),
            json!(self.game.bricks.iter().filter(|live| **live).count() as i64),
        );
        values.insert("game.score".to_string(), json!(self.game.score));
        values.insert("game.lives".to_string(), json!(self.game.lives));
        values.insert("cells.A1".to_string(), json!(self.grid_value(1, 1)));
        values.insert("cells.A2".to_string(), json!(self.grid_value(2, 1)));
        values.insert("cells.A3".to_string(), json!(self.grid_value(3, 1)));
        values.insert("cells.B1".to_string(), json!(self.grid_value(1, 2)));
        values.insert("cells.B2".to_string(), json!(self.grid_value(2, 2)));
        for (row, col, name) in [
            (1, 1, "A1"),
            (2, 1, "A2"),
            (3, 1, "A3"),
            (1, 2, "B1"),
            (2, 2, "B2"),
        ] {
            values.insert(
                format!("cells.{name}.formula"),
                json!(self.grid_text(row, col)),
            );
        }
        values.insert(
            "cells.selected_formula".to_string(),
            json!(self.grid_text(self.grid.selected.0, self.grid.selected.1)),
        );
        values.insert(
            "cells.selected_value".to_string(),
            json!(self.grid_value(self.grid.selected.0, self.grid.selected.1)),
        );
        values.insert(
            "cells.selected".to_string(),
            json!(format!(
                "{}{}",
                column_name(self.grid.selected.1),
                self.grid.selected.0
            )),
        );
        values.insert(
            "cells.editing".to_string(),
            json!(self.grid.editing.map(|(row, col)| format!("{}{}", column_name(col), row))),
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
fn rects_overlap(
    ax: i64,
    ay: i64,
    aw: i64,
    ah: i64,
    bx: i64,
    by: i64,
    bw: i64,
    bh: i64,
) -> bool {
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
