# Boon Rust Implementation Plan

This file is the implementation brief for the new `boon-rust` repository. It is written for Codex CLI / AI implementation agents and should be treated as the source of truth unless a human maintainer explicitly updates it.

The goal is a simple, strict, deterministic, fast Boon-to-Rust compiler/runtime with three verified rendering targets:

1. **Ratatui** for terminal apps, CI, AI-debuggable snapshots, and PTY tests.
2. **Native WebGPU/wgpu** using `app_window`, WESL, `wgsl_bindgen`, and framebuffer readback verification.
3. **Browser WebGPU/wgpu** using the same generated shaders and Playwright-driven verification in Chromium and Firefox.

The current focus examples are:

- `counter`
- `counter_hold`
- `interval`
- `interval_hold`
- `todo_mvc`
- `todo_mvc_physical`
- `cells`
- `pong`
- `arkanoid`

All examples must eventually run and be automatically verified on all three backends.

---

## 0. Non-negotiable design constraints

### 0.1 Do not redesign Boon syntax

Do **not** introduce new user-facing syntax or concepts while implementing this repo.

Do not add:

- `CASE`
- `ON`
- `List/State/*`
- `List/View/*`
- `List/Aggregate/*`
- `Sheet/new`
- `Element/Button/source()`
- nominal source/capability types
- typed source constructors
- source classes/interfaces in Boon modules
- Rust async/futures as Boon's internal execution model
- runtime streams/channels for every Boon stream
- winit fallback
- Sokol
- SDL3
- Slang in the initial renderer pipeline
- `wgsl_to_wgpu`; use `wgsl_bindgen`

If nested module paths are missing in the parser, implement nested module paths. Nested module paths are intended Boon design, not a reason to flatten APIs.

### 0.2 Keep Boon code pure data

Boon business/app code should remain pure data plus stream-like expressions.

Important principles:

- Modules are files / module paths, not nominal type containers.
- `PASS` / `PASSED` remain valid and important.
- `WHEN`, `THEN`, `WHILE`, `HOLD`, `LATEST`, `LIST`, `BLOCK`, tags, records, text, numbers, and function/module calls remain the core user-facing model.
- From the user's perspective, there is no hard value/event distinction.
- From the user's perspective, there is no explicit `Bool` type. `True` and `False` are tags.
- From the user's perspective, there is no explicit `Pulse` type. A click/press/change can be represented internally as a stream that emits `[]`.
- Everything is stream-like enough that existing Boon expressions should keep working.

### 0.3 `SOURCE` is a pure data marker

`SOURCE` is a user-visible marker in a Boon data record saying:

> The host/runtime must produce this stream at this data path.

`SOURCE` is not:

- a source constructor,
- a nominal source type,
- an unsafe escape hatch,
- a capability object,
- a Rust object,
- a runtime stream object.

A source declaration should look like ordinary Boon data:

```boon
store: [
    sources: [
        increment_button: [
            event: [
                press: SOURCE
            ]
            hovered: SOURCE
        ]
    ]
]
```

The same source record is passed into the document tree:

```boon
document:
    Document/new(
        root:
            Element/button(
                element: store.sources.increment_button
                style: []
                label: counter |> Text/from_number()
            )
    )
```

And business logic reads the same path:

```boon
counter:
    0 |> HOLD state {
        store.sources.increment_button.event.press
        |> THEN { state + 1 }
    }
```

This is the replacement for the old `|> LINK { ... }` pattern. Old `LINK` solved two related problems:

1. How business logic refers to elements/events/states in the document tree.
2. How runtime events/state flow into Boon values.

`SOURCE` keeps both capabilities, but reverses direction:

- Old: an element/link was created and then linked outward to a Boon variable/path.
- New: the Boon path/source record exists first as data, and the element binds it.

### 0.4 Compiler must be strict

Prefer compile errors over warnings whenever possible.

The compiler must error when:

- A `SOURCE` leaf is never bound by any host/runtime producer.
- A `SOURCE` leaf is bound by more than one live producer.
- A `SOURCE` leaf receives conflicting structural value shapes.
- A source record passed to a host function contains unknown `SOURCE` leaves that the host function cannot bind.
- A source record passed to a host function is missing required source leaves for that host function.
- A source path is read by logic but cannot be proven to be produced by the host/runtime.
- A source path is produced by a host binding but is used in a shape-incompatible expression.
- A dynamic `SOURCE` owner cannot be proven stable.
- A dynamic source event references a removed list item with a stale generation.
- A source path is typo-like or unknown.

The compiler may warn, rather than error, only for purely diagnostic issues that cannot affect runtime correctness. Prefer eventually upgrading warnings to errors.

### 0.5 No async/channels/streams inside Boon runtime

The generated Boon runtime must be a deterministic synchronous turn machine.

Do not model Boon internals as:

- Rust `async` tasks,
- `Future`s,
- `Stream`s,
- Tokio channels,
- crossbeam channels per Boon edge,
- dynamic runtime stream subscription graphs,
- wakers,
- mutex/atomic-heavy graph execution.

Async is allowed at host boundaries only:

- browser event loop,
- `app_window` integration,
- wgpu initialization/submission/readback where needed,
- Playwright/browser test driver,
- file/network/device adapters if future examples need them.

Inside a generated Boon app:

```text
source emission
  -> one deterministic turn
  -> state updates
  -> dirty derived recomputation
  -> one render patch batch
  -> backend applies/draws
```

---

## 1. Workspace layout

Use a Rust workspace.

Recommended crate layout:

```text
boon-rust/
  Cargo.toml
  crates/
    boon_syntax/
    boon_hir/
    boon_shape/
    boon_host_schema/
    boon_source/
    boon_runtime/
    boon_compiler/
    boon_codegen_rust/
    boon_render_ir/
    boon_backend_ratatui/
    boon_backend_wgpu/
    boon_backend_app_window/
    boon_backend_browser/
    boon_examples/
    boon_verify/
    xtask/
  examples/
    counter/
    counter_hold/
    interval/
    interval_hold/
    todo_mvc/
    todo_mvc_physical/
    cells/
    pong/
    arkanoid/
  shaders/
    common/
    pipelines/
  tests/
    scenarios/
  docs/
```

Crate responsibilities:

### `boon_syntax`

Parser, spans, source files, parser diagnostics.

Must support:

- records `[a: b]`
- nested records
- text literals `TEXT { ... }`
- tags `True`, `False`, `Enter`, etc.
- `LIST { ... }`
- `BLOCK { ... }`
- `HOLD`, `LATEST`, `WHEN`, `THEN`, `WHILE`
- pipe operator `|>`
- function calls
- nested module paths, e.g. `Element/button`, `Math/sum`, `Router/route`, and deeper paths if examples use them
- `PASS` / `PASSED`
- `SOURCE`

Do not simplify the language by dropping constructs from the original examples.

### `boon_hir`

Typed/validated high-level IR, symbol table, module/file graph, resolved names, lowered `PASS` / `PASSED` environments.

This should preserve enough original structure for diagnostics.

### `boon_shape`

Internal structural shape inference.

This is not user-facing nominal typing.

Shapes can include:

```text
Unknown
EmptyRecord                 // [] emission for press/click/change/blur/etc.
Record(fields)
List(item_shape)
Text
Number
TagSet(tags)                // e.g. {True, False}, {Enter, Escape, ...}
Function(...)
SourceMarker
Skip
Union/Maybe if needed internally
```

Do not introduce user-facing `Bool` or `Pulse`.

Use `TagSet { True, False }` internally for booleans.
Use `EmptyRecord` internally for press/click/change/blur/focus emissions.

### `boon_host_schema`

Structural contracts for runtime/host functions.

Examples:

```text
Element/button(element)
  element.event.press -> EmptyRecord
  element.hovered     -> TagSet { False, True } optional
  element.focused     -> TagSet { False, True } optional

Element/text_input(element)
  element.text               -> Text required
  element.event.change       -> EmptyRecord required
  element.event.key_down.key -> Key tags required
  element.event.blur         -> EmptyRecord optional
  element.event.focus        -> EmptyRecord optional

Element/checkbox(element)
  element.event.click -> EmptyRecord required
  element.checked     -> TagSet { False, True } optional or required depending on widget semantics
  element.hovered     -> TagSet { False, True } optional

Element/label(element)
  element.event.double_click -> EmptyRecord optional
  element.hovered            -> TagSet { False, True } optional
```

The exact contract is maintained in Rust data, not Boon nominal source declarations.

If an element accepts a source record, every `SOURCE` leaf in that record must be accounted for by that host function's source contract. Unknown source fields are errors.

### `boon_source`

Resolves every `SOURCE` leaf to:

- static source slot, or
- dynamic source family under a stable owner, usually list item identity.

Tracks:

- source path,
- structural value shape,
- host producer/binder,
- logic readers,
- render node bindings,
- dynamic owner kind,
- generation checking for dynamic sources.

### `boon_runtime`

Tiny deterministic runtime primitives:

- source event dispatch,
- turn execution,
- dirty sets,
- keyed list arenas,
- source family generation checking,
- render patch buffer,
- fake/real clock abstractions,
- snapshots and replay support.

No async runtime inside this crate.

### `boon_compiler`

End-to-end compiler orchestration:

```text
parse
  -> HIR
  -> shape inference
  -> host source binding validation
  -> source inventory
  -> dependency graph
  -> list/grid optimizations
  -> render IR lowering
  -> codegen request
```

Also expose a reference interpreter for testing generated code.

### `boon_codegen_rust`

Generates Rust code for Boon examples/apps.

Output should be direct, boring Rust:

- structs for state,
- enums for source events,
- source slots/families,
- direct handlers,
- dirty recomputation functions,
- render patch emission,
- no async/channels internally.

### `boon_render_ir`

Backend-independent semantic render patches and optional lower-level draw IR.

Two layers:

```text
HostViewIR
  semantic UI tree: text, lists, buttons, checkboxes, grid, panels, source bindings

RayboxDrawIR
  drawing primitives: rects, lines, text runs, rounded boxes, transforms, clips, materials
```

Ratatui consumes mostly HostViewIR.
GPU backends lower HostViewIR to RayboxDrawIR and then to wgpu.

### `boon_backend_ratatui`

Terminal renderer for all examples.

Must support:

- Ratatui rendering,
- in-memory buffer tests,
- PTY integration tests,
- keyboard simulation,
- deterministic frame snapshots,
- all examples including `cells`, `pong`, `arkanoid`, and debug projection for `todo_mvc_physical`.

### `boon_backend_wgpu`

Shared wgpu renderer core.

Must support:

- WESL -> WGSL -> `wgsl_bindgen` shader pipeline,
- offscreen framebuffer target for tests,
- native and browser use,
- shared pipelines for UI/grid/text/physical/debug draw,
- renderer timing instrumentation,
- framebuffer readback or deterministic frame hash.

Do not depend on `winit`, Sokol, SDL3, or Slang.

### `boon_backend_app_window`

Native window integration using `app_window`, not winit.

Responsibilities:

- create native window/surface,
- connect input to source emissions,
- drive app loop,
- present frames from `boon_backend_wgpu`,
- provide app-window smoke tests.

No winit fallback.

### `boon_backend_browser`

Browser wasm runner.

Responsibilities:

- initialize wgpu in browser,
- connect browser/canvas input to source emissions,
- expose test-only JS API for Playwright,
- return state, source inventory, frame hashes/frames, metrics.

### `boon_examples`

Example app definitions and generated runners.

Each example must have:

- Boon source file,
- generated app module,
- Ratatui runner,
- native wgpu runner,
- browser wgpu runner,
- verification scenarios.

### `boon_verify`

Common verification DSL and backend implementations.

### `xtask`

Developer/CI command runner.

Commands should include:

```bash
cargo xtask verify all
cargo xtask verify ratatui
cargo xtask verify ratatui --pty
cargo xtask verify native-wgpu --headless
cargo xtask verify native-wgpu --app-window
cargo xtask verify browser-wgpu --browser chromium
cargo xtask verify browser-wgpu --browser firefox
cargo xtask bench todo_mvc --backend all --todos 100
cargo xtask bench cells --backend all --rows 100 --cols 26
cargo xtask shaders
cargo xtask generate
```

---

## 2. Core execution architecture

### 2.1 Deterministic turn machine

The generated app must run as a deterministic turn machine.

One external source emission creates one logical turn:

```text
external source emission
  -> dispatch source event
  -> snapshot current committed state for the turn
  -> run handlers/updates
  -> update state
  -> recompute dirty derived values
  -> produce one render patch batch
  -> return idle
```

No work should remain in hidden async tasks inside the app.

### 2.2 Turn snapshot rule

Within one turn:

- `HOLD state` reads the previous committed value for that state cell.
- `THEN` samples current values at the event boundary according to Boon semantics.
- Global values used by many list items must be sampled consistently.
- Bulk updates such as TodoMVC `toggle_all` must use the same sampled `store.all_completed` for every todo.

This avoids the fragile bug where `all_completed` changes midway through toggling 100 todos.

### 2.3 Generated Rust shape

Generated Rust should look like hand-written event code.

Minimal counter conceptual output:

```rust
pub struct App {
    state: State,
    sources: Sources,
    dirty: Dirty,
    patches: Vec<HostPatch>,
}

struct State {
    counter: i64,
}

struct Sources {
    increment_button_hovered: TagFalseTrue,
}

pub enum SourceEvent {
    StoreSourcesIncrementButtonEventPress,
    StoreSourcesIncrementButtonHovered(TagFalseTrue),
}

impl App {
    pub fn dispatch_source(&mut self, event: SourceEvent) {
        let snapshot = self.snapshot_for_turn();

        match event {
            SourceEvent::StoreSourcesIncrementButtonEventPress => {
                self.state.counter = snapshot.counter + 1;
                self.dirty.counter = true;
            }

            SourceEvent::StoreSourcesIncrementButtonHovered(value) => {
                self.sources.increment_button_hovered = value;
                self.dirty.increment_button_style = true;
            }
        }

        self.recompute_dirty();
        self.emit_render_patches();
    }
}
```

The exact generated code may differ, but the hot path must be direct and predictable.

### 2.4 Source event representation

Static sources become enum variants or numeric slots.

Dynamic sources become source families keyed by stable owner IDs and generations.

Conceptual Rust:

```rust
pub enum SourceEvent {
    Static {
        slot: StaticSourceSlot,
        value: BoonValue,
    },
    Dynamic {
        family: SourceFamilyId,
        owner: OwnerId,
        generation: u32,
        value: BoonValue,
    },
}
```

Optimized generated code may specialize events:

```rust
pub enum AppSourceEvent {
    StoreNewTodoInputText(Text),
    StoreNewTodoInputChange,
    StoreNewTodoInputKeyDownKey(KeyTag),
    StoreToggleAllCheckboxClick,
    TodoCheckboxClick(TodoId, Generation),
    TodoRemoveButtonPress(TodoId, Generation),
    TodoEditInputText(TodoId, Generation, Text),
    TodoEditInputChange(TodoId, Generation),
}
```

Human-readable source paths are retained in manifests and traces, not in hot dispatch.

---

## 3. SOURCE resolution and strict host binding

### 3.1 Host contracts are structural

Host contracts are Rust-side schema data, not Boon declarations.

Example conceptual contract:

```rust
HostContract::new("Element/button")
    .source_arg("element")
    .optional("event.press", Shape::EmptyRecord)
    .optional("hovered", Shape::tag_set(["False", "True"]))
    .optional("focused", Shape::tag_set(["False", "True"]));
```

For text input:

```rust
HostContract::new("Element/text_input")
    .source_arg("element")
    .required("text", Shape::Text)
    .required("event.change", Shape::EmptyRecord)
    .required("event.key_down.key", Shape::key_tags())
    .optional("event.blur", Shape::EmptyRecord)
    .optional("event.focus", Shape::EmptyRecord)
    .optional("focused", Shape::tag_set(["False", "True"]));
```

For checkbox:

```rust
HostContract::new("Element/checkbox")
    .source_arg("element")
    .required("event.click", Shape::EmptyRecord)
    .optional("checked", Shape::tag_set(["False", "True"]))
    .optional("hovered", Shape::tag_set(["False", "True"]));
```

If a host function does not produce a source path, the compiler must not assume it exists.

### 3.2 Button example

Boon:

```boon
store: [
    sources: [
        increment_button: [
            event: [
                press: SOURCE
            ]
            hovered: SOURCE
        ]
    ]
]

counter:
    0 |> HOLD state {
        store.sources.increment_button.event.press
        |> THEN { state + 1 }
    }

document:
    Document/new(
        root:
            Element/button(
                element: store.sources.increment_button
                style: [
                    background:
                        store.sources.increment_button.hovered
                        |> WHEN {
                            True => Oklch [lightness: 0.8 chroma: 0.1 hue: 120]
                            False => Oklch [lightness: 0.95 chroma: 0.02 hue: 120]
                        }
                ]
                label: counter |> Text/from_number()
            )
    )
```

Compiler source inventory:

```text
store.sources.increment_button.event.press
  shape: []
  producer: Element/button(element.event.press)
  readers: counter.HOLD

store.sources.increment_button.hovered
  shape: tags { False, True }
  producer: Element/button(element.hovered)
  readers: button style background expression
```

### 3.3 Text input example

Do not merge `key_down` with `text`.

Good:

```boon
store: [
    sources: [
        new_todo_input: [
            text: SOURCE
            event: [
                change: SOURCE
                key_down: [
                    key: SOURCE
                ]
                blur: SOURCE
                focus: SOURCE
            ]
        ]
    ]

    title_to_add:
        sources.new_todo_input.event.key_down.key
        |> WHEN {
            Enter => BLOCK {
                trimmed:
                    sources.new_todo_input.text
                    |> Text/trim()

                trimmed
                |> Text/is_not_empty()
                |> WHEN {
                    True => trimmed
                    False => SKIP
                }
            }

            __ => SKIP
        }
]
```

`key_down.key` only produces a key tag. It never carries text.

`source.text` is the current text stream/value from the runtime.

`change` emits `[]` when text changed.

### 3.4 Dynamic source families

Inside a list item constructor:

```boon
FUNCTION new_todo(title) {
    [
        sources: [
            checkbox: [
                event: [
                    click: SOURCE
                ]
            ]

            remove_button: [
                event: [
                    press: SOURCE
                ]
                hovered: SOURCE
            ]

            edit_input: [
                text: SOURCE
                event: [
                    change: SOURCE
                    key_down: [
                        key: SOURCE
                    ]
                    blur: SOURCE
                ]
            ]
        ]

        completed:
            False |> HOLD state {
                sources.checkbox.event.click
                |> THEN { state |> Bool/not() }
            }
    ]
}
```

When `new_todo` is used inside `LIST` / `List/append`, these source paths become source families:

```text
todos[*].sources.checkbox.event.click
todos[*].sources.remove_button.event.press
todos[*].sources.edit_input.text
todos[*].sources.edit_input.event.change
```

Runtime representation:

```rust
struct DynamicSourceRef {
    family: SourceFamilyId,
    owner: TodoId,
    generation: u32,
}
```

If a todo is removed, its generation is invalidated. Late events from removed UI rows are ignored or surfaced as debug errors in test builds.

---

## 4. List and dynamic item lowering

### 4.1 Keep current list APIs

Do not introduce public `List/State`, `List/View`, or `List/Aggregate` modules for now.

User code can keep current style:

```boon
todos:
    LIST {
        new_todo(title: TEXT { Buy groceries })
        new_todo(title: TEXT { Clean room })
    }
    |> List/append(
        item:
            title_to_add
            |> new_todo(title: PASSED)
    )
    |> List/remove(
        item,
        on: item.sources.remove_button.event.press
    )
    |> List/remove(
        item,
        on:
            sources.clear_completed_button.event.press
            |> THEN {
                item.completed
                |> WHEN {
                    True => []
                    False => SKIP
                }
            }
    )
```

The compiler must optimize known structural patterns.

### 4.2 Owned list representation

Dynamic lists should not become runtime stream graphs.

Use owned arenas/keyed vectors:

```rust
struct TodoList {
    items: SlotVec<TodoId, Todo>,
    order: Vec<TodoId>,
    generations: Vec<u32>,
}

struct Todo {
    title: Text,
    editing: TagFalseTrue,
    completed: TagFalseTrue,
    sources: TodoSourceState,
}
```

Do not use one mailbox or channel per item.

### 4.3 Item-local remove

Pattern:

```boon
List/remove(item, on: item.sources.remove_button.event.press)
```

Lower to:

```rust
fn on_todo_remove_button_press(&mut self, id: TodoId, generation: u32) {
    if !self.todos.is_live(id, generation) {
        return;
    }

    self.remove_todo(id);
}
```

### 4.4 Clear completed

Pattern:

```boon
List/remove(
    item,
    on:
        sources.clear_completed_button.event.press
        |> THEN {
            item.completed
            |> WHEN {
                True => []
                False => SKIP
            }
        }
)
```

Lower to one bulk remove:

```rust
fn on_clear_completed_press(&mut self) {
    let removed = self.todos.remove_where(|todo| todo.completed == Tag::True);
    self.completed_count = 0;
    self.active_count = self.todos.len();
    self.all_completed = self.completed_count == self.todos.len();
    self.render.remove_todo_rows(&removed);
    self.render.patch_footer_counts(self.active_count, self.completed_count);
}
```

No per-item dynamic subscription storm.

### 4.5 Toggle all

User code may remain inside each todo:

```boon
completed:
    False |> HOLD state {
        LATEST {
            sources.checkbox.event.click
            |> THEN {
                state |> Bool/not()
            }

            store.sources.toggle_all_checkbox.event.click
            |> THEN {
                store.all_completed |> Bool/not()
            }
        }
    }
```

Compiler must recognize:

```text
global source used inside each dynamic item field update
same target field: todo.completed
same sampled expression: !store.all_completed
```

Lower to one bulk handler:

```rust
fn on_toggle_all_checkbox_click(&mut self) {
    let target = not(self.turn_snapshot.all_completed);

    for id in self.todos.order.iter().copied() {
        self.todos.items[id].completed = target;
    }

    self.completed_count = if target == Tag::True {
        self.todos.len()
    } else {
        0
    };
    self.active_count = self.todos.len() - self.completed_count;
    self.all_completed = self.completed_count == self.todos.len();

    self.render.patch_all_visible_todo_completion(target);
    self.render.patch_footer_counts(self.active_count, self.completed_count);
    self.render.patch_toggle_all_state(self.all_completed);
}
```

The 100-todo toggle-all target is under 10ms source-to-complete-frame.

### 4.6 Aggregates

Current user code:

```boon
todos_count:
    todos |> List/count()

completed_todos_count:
    todos
    |> List/retain(item, if: item.completed)
    |> List/count()

active_todos_count:
    todos_count - completed_todos_count

all_completed:
    todos_count == completed_todos_count
```

Compiler should not materialize `List/retain` just to count.

Lower to maintained counters:

```rust
todos_count = todos.len();
completed_todos_count = maintained counter;
active_todos_count = todos_count - completed_todos_count;
all_completed = todos_count == completed_todos_count;
```

When one todo changes completion:

```rust
fn update_completed_delta(&mut self, old: TagFalseTrue, new: TagFalseTrue) {
    if old == new { return; }
    match (old, new) {
        (False, True) => self.completed_count += 1,
        (True, False) => self.completed_count -= 1,
        _ => {}
    }
    self.active_count = self.todos.len() - self.completed_count;
    self.all_completed = self.completed_count == self.todos.len();
}
```

### 4.7 Derived visible lists

Patterns like:

```boon
todos
|> List/retain(item, if: selected_filter |> WHEN { ... })
|> List/map(item, new: todo_item(todo: item))
```

Should lower to keyed projections, not full teardown/rebuild on every item update.

Maintain:

```rust
visible: Vec<TodoId>
row_instances: HashMap<TodoId, RenderNodeId>
```

On filter change, diff keyed lists.

On one checkbox update, update membership for that item only when possible.

---

## 5. Cells without `Sheet/new`

Do not introduce `Sheet/new` into Boon std or language.

The cells example should be fast because the compiler/runtime understands generic dense indexed structures, source families, dirty derived values, and dependency graphs — not because the language has a spreadsheet builtin.

### 5.1 Detect dense indexed collection shapes

Look for structural patterns like:

```boon
List/range(from: 1, to: 100)
|> List/map(row, new:
    make_row_data(
        row_number: row
        row_cells: make_row_cells(row: row)
    )
)
```

and nested static ranges for columns.

Lower static `range × range` constructions to dense arrays with stable `CellId`:

```rust
const ROWS: usize = 100;
const COLS: usize = 26;
const CELL_COUNT: usize = ROWS * COLS;

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
struct CellId(u16);
```

### 5.2 Source families per cell

Cell editor/display sources become source families:

```text
cells[*].sources.display.event.double_click
cells[*].sources.editor.text
cells[*].sources.editor.event.change
cells[*].sources.editor.event.key_down.key
cells[*].sources.editor.event.blur
```

Generated events:

```rust
pub enum CellSourceEvent {
    DisplayDoubleClick(CellId, Generation),
    EditorText(CellId, Generation, Text),
    EditorChange(CellId, Generation),
    EditorKeyDownKey(CellId, Generation, KeyTag),
    EditorBlur(CellId, Generation),
}
```

### 5.3 Formula/cache/dependency model

Use a spreadsheet-like internal engine, but do not expose it as `Sheet/new`.

Conceptual Rust:

```rust
struct CellsState {
    formulas: Vec<FormulaAst>,
    formula_text: Vec<Text>,
    values: Vec<CellValue>,
    deps: Vec<SmallVec<[Dependency; 4]>>,
    rev_deps: Vec<SmallVec<[CellId; 4]>>,
    dirty: BitSet,
    editing: EditingState,
    overrides: HashMap<CellId, Text>,
}
```

Editing a cell:

```rust
fn commit_cell_formula(&mut self, cell: CellId, text: Text) {
    self.update_override(cell, text);
    self.reparse_formula(cell);
    self.update_dependencies(cell);
    self.mark_cell_and_dependents_dirty(cell);
    self.recalculate_dirty_cells();
    self.emit_cell_patches();
}
```

Do not recalculate all 2,600 cells when one cell changes.

### 5.4 Range formulas

Initially, direct dependencies for small ranges are acceptable for 26×100.

For larger scales or performance issues, add range aggregate nodes:

```rust
struct RangeNode {
    range: CellRange,
    aggregate: CellValue,
    dependents: SmallVec<[CellId; 4]>,
}
```

Possible future optimizations:

- prefix sums,
- Fenwick tree,
- segment tree,
- cached range nodes.

### 5.5 Cells rendering expectations

All three backends must show a nice Excel-like grid:

- column headers A-Z,
- row numbers,
- selected cell,
- editing overlay/state,
- grid lines/borders,
- formula/value display,
- deterministic error rendering for invalid formulas/cycles,
- scrolling or viewport behavior if needed.

Ratatui should render a usable text grid.
GPU should render crisp grid lines and text.

---

## 6. Interval, interval_hold, pong, arkanoid: deterministic time

All time-based examples must use an abstract clock source.

Never rely on real sleeping in tests.

```rust
trait ClockSource {
    fn now(&self) -> Time;
}

struct FakeClock {
    now: Time,
}

impl FakeClock {
    fn advance(&mut self, delta: Duration) { ... }
}
```

`interval` and `interval_hold` verification must run by advancing fake time.

`pong` and `arkanoid` must run deterministic frame steps with seeded/random-free inputs.

Tests must be able to assert exact positions/values after N ticks/frames.

---

## 7. Render architecture

### 7.1 Common patch IR

Boon app should produce backend-independent patches.

Example patch enum:

```rust
pub enum HostPatch {
    CreateNode { id: NodeId, kind: NodeKind, parent: Option<NodeId>, key: Option<Key> },
    RemoveNode { id: NodeId },
    MoveNode { id: NodeId, parent: NodeId, index: usize },
    SetText { id: NodeId, text: Text },
    SetTag { id: NodeId, tag: Tag },
    SetStyle { id: NodeId, patch: StylePatch },
    SetLayout { id: NodeId, patch: LayoutPatch },
    SetSourceBinding { id: NodeId, binding: SourceBinding },
    SetGridCell { id: NodeId, row: usize, col: usize, value: Text },
    SetGeometry { id: NodeId, patch: GeometryPatch },
}
```

Keep patches semantic enough for Ratatui and structured enough for GPU lowering.

### 7.2 HostViewIR and RayboxDrawIR

Use two layers:

```text
HostViewIR
  semantic tree: buttons, labels, inputs, checkboxes, lists, grids, panels, source bindings

RayboxDrawIR
  draw primitives: rects, text runs, lines, clips, transforms, materials, SDF/physical primitives
```

Ratatui consumes HostViewIR.
Native/browser GPU lower HostViewIR to RayboxDrawIR.

### 7.3 Backend trait

Conceptual trait:

```rust
pub trait Backend {
    fn apply_patches(&mut self, patches: &[HostPatch]);
    fn push_source_emissions(&mut self, out: &mut Vec<SourceEmission>);
    fn render_frame(&mut self) -> anyhow::Result<FrameInfo>;
    fn capture_frame(&mut self) -> anyhow::Result<FrameSnapshot>;
    fn metrics(&self) -> BackendMetrics;
}
```

The exact trait may differ between sync/async frontends, but the Boon core must remain synchronous.

---

## 8. Ratatui backend

Ratatui is the primary correctness/debug backend.

All examples must run in Ratatui.

### 8.1 Why Ratatui

Ratatui supports test backends and terminal rendering. It is good for AI debugging because terminal frames are plain text and can be snapshotted. PTY tests catch real terminal behavior.

### 8.2 Required modes

1. **Buffer mode** using Ratatui `TestBackend` or direct buffer rendering.
2. **PTY mode** using `portable-pty`.

### 8.3 Buffer tests

Use in-memory frame snapshots:

```text
render frame
capture buffer
compare snapshot
```

Good for most regression tests.

### 8.4 PTY tests

Use `portable-pty` to spawn real terminal example binaries.

PTY flow:

```text
spawn example binary in PTY
set terminal size
send keyboard input bytes
read terminal output
parse terminal screen buffer
compare final screen
measure input -> visible update latency
```

Artifacts:

```text
target/boon-artifacts/<example>/ratatui/
  frames.txt
  trace.json
  ansi.log
  timings.json
```

### 8.5 Ratatui rendering for examples

- `counter`: button/key plus value.
- `counter_hold`: same.
- `interval`: ticking value with fake clock in tests.
- `interval_hold`: accumulated/held value with fake clock.
- `todo_mvc`: input row, todos, checkbox markers, filters, footer.
- `todo_mvc_physical`: debug semantic/physical projection with depth/order/bounds/source info.
- `cells`: spreadsheet grid with headers and selected/editing cell.
- `pong`: character grid/blocks.
- `arkanoid`: character grid/blocks/bricks.

---

## 9. Native wgpu backend

Native GPU rendering uses:

- `app_window` for window management,
- `wgpu` for graphics,
- WESL for shader modularity,
- `wgsl_bindgen` for Rust/wgpu shader bindings.

No winit fallback.
No Sokol.
No SDL3.
No Slang initially.

### 9.1 Native does not require installing WebGPU

Native `wgpu` uses system GPU APIs such as Vulkan, Metal, D3D12, or OpenGL depending on platform and driver support. Do not require users to install a separate “WebGPU runtime.”

Users still need normal working GPU drivers/windowing stack.

### 9.2 app_window

Use `app_window` instead of winit.

Reasons:

- async-first window API,
- cross-platform native/web intent,
- raw-window-handle integration for wgpu,
- Wayland support on Linux.

The renderer core must not depend directly on app_window. Keep a host layer:

```rust
struct AppWindowHost { ... }
struct WgpuRenderer { ... }
```

### 9.3 Offscreen framebuffer verification

Do not use OS-native screenshots.

Every GPU frame should render into an owned frame texture first:

```text
Raybox frame texture
  -> readback for tests
  -> present to app_window surface
```

Native tests should support a headless/offscreen mode:

```text
native_wgpu_headless
  no window
  render to offscreen texture
  copy texture to buffer
  compare pixels/hash/layout probes
```

And an app-window smoke mode:

```text
native_wgpu_app_window
  real app_window surface
  still capture internal frame texture before present
```

---

## 10. Browser wgpu backend

Browser GPU rendering uses:

- Rust wasm,
- wgpu browser/WebGPU backend,
- same WESL-generated WGSL roots,
- same or near-same `wgsl_bindgen` generated Rust modules,
- Playwright for Chromium and Firefox verification.

### 10.1 Test API

Expose a test-only browser API:

```ts
window.__boonTest = {
  send(action),
  runUntilIdle(),
  inspectState(path),
  inspectSources(),
  inspectViewTree(),
  captureFrameRgba(),
  captureFrameHash(),
  metrics(),
}
```

This API must be compiled only for test/dev builds.

### 10.2 Browser verification

Use Playwright to drive Chromium and Firefox.

Browser tests must:

```text
serve wasm app
open page
wait for wgpu ready
send scenario actions
run until idle
inspect state
capture frame/hash from app test API
collect timings
store artifacts
```

Do not rely only on `page.screenshot()` for correctness. Browser screenshots are useful artifacts, but strict tests should use internal frame readback/hash and semantic state inspection.

Artifacts:

```text
target/boon-artifacts/<example>/browser-chromium/
  frame_000.png
  trace.json
  timings.json
  playwright-trace.zip

target/boon-artifacts/<example>/browser-firefox/
  frame_000.png
  trace.json
  timings.json
  playwright-trace.zip
```

---

## 11. Shader pipeline

Use:

```text
WESL source
  -> build.rs / xtask compiles WESL to WGSL
  -> wgsl_bindgen generates Rust bindings
  -> wgpu renderer uses generated modules
```

Do not use Slang initially.
Do not use `wgsl_to_wgpu`.
Do not hand-write bind group layouts that duplicate WGSL.

### 11.1 Why WESL

Plain WGSL is acceptable for small shaders but gets hard to maintain as Raybox grows. WESL keeps WGSL/WebGPU compatibility while adding module/import organization.

Use WESL for:

```text
common math
layout/geometry helpers
SDF primitives
text/glyph helpers
materials
debug visualizations
pipeline roots
```

### 11.2 Why wgsl_bindgen

`wgsl_bindgen` generates type-safe Rust bindings from WGSL for wgpu. This reduces shader/Rust binding drift.

Use generated bindings as the only way to create:

- shader modules,
- bind group layouts,
- bind groups,
- pipeline constants,
- uniform/storage buffer bindings where supported.

### 11.3 Build structure

Suggested shader layout:

```text
shaders/
  common/
    math.wesl
    color.wesl
    text.wesl
    sdf.wesl
    ui.wesl
  pipelines/
    ui_rects.wesl
    ui_text.wesl
    grid.wesl
    physical_debug.wesl
    present.wesl
```

`build.rs` or `xtask shaders` should:

1. Compile WESL roots to generated WGSL files in `OUT_DIR` or `target/generated-shaders`.
2. Run `wgsl_bindgen` on the generated WGSL roots.
3. Generate one Rust module included by `boon_backend_wgpu`.
4. Fail fast on shader validation errors.

---

## 12. Verification framework

### 12.1 One scenario DSL for all platforms

Do not write separate test logic for each backend.

Define one scenario format, e.g. RON/JSON/YAML or Rust builder.

Conceptual example:

```ron
ExampleScenario(
    name: "todo_mvc_add_toggle_clear",
    example: "todo_mvc",
    viewport: (120, 40),
    seed: 1,
    steps: [
        ExpectText("What needs to be done?"),

        TypeText(source: "store.sources.new_todo_input", text: "Buy milk"),
        PressKey(source: "store.sources.new_todo_input.event.key_down.key", key: Enter),
        RunUntilIdle,
        ExpectText("Buy milk"),
        ExpectState(path: "store.todos_count", equals: "3"),

        Click(source: "store.todos[0].sources.checkbox.event.click"),
        RunUntilIdle,
        ExpectText("2 items left"),

        Click(source: "store.sources.toggle_all_checkbox.event.click"),
        RunUntilIdle,
        ExpectState(path: "store.completed_todos_count", equals: "3"),
        ExpectFrameBudget(max_ms: 10.0),
    ],
)
```

Backends implement:

```rust
trait VerificationBackend {
    fn load_example(&mut self, name: &str, viewport: Size);
    fn send(&mut self, action: TestAction);
    fn run_until_idle(&mut self);
    fn inspect_state(&self, path: &str) -> InspectValue;
    fn inspect_sources(&self) -> SourceInventory;
    fn inspect_view_tree(&self) -> ViewSnapshot;
    fn capture_frame(&mut self) -> FrameSnapshot;
    fn metrics(&self) -> BackendMetrics;
}
```

### 12.2 What tests must verify

For each platform:

- semantic state,
- source inventory and binding correctness,
- visual/frame output,
- interaction behavior,
- timing metrics,
- stale dynamic source protection,
- deterministic replay.

### 12.3 Test commands

Implement:

```bash
cargo xtask verify all
cargo xtask verify ratatui
cargo xtask verify ratatui --pty
cargo xtask verify native-wgpu --headless
cargo xtask verify native-wgpu --app-window
cargo xtask verify browser-wgpu --browser chromium
cargo xtask verify browser-wgpu --browser firefox
cargo xtask bench todo_mvc --backend all --todos 100
cargo xtask bench cells --backend all --rows 100 --cols 26
```

---

## 13. Performance targets

All timing should use source-event-to-complete-frame where possible:

```text
source event received
  -> Boon state update
  -> dirty recomputation
  -> render patch generation
  -> backend patch application
  -> final terminal buffer or GPU frame texture ready
```

### 13.1 TodoMVC typing

Human target:

> Holding a letter in the TodoMVC input must not visibly lag.

Test:

```text
type 100 repeated characters into new_todo_input
measure per-key source event -> complete frame
verify final text exactly matches expected
```

Budgets:

```text
p95 <= 8 ms
p99 <= 16 ms
no dropped logical characters
```

### 13.2 TodoMVC checking one item

Target:

```text
100 todos loaded
click one checkbox
source event -> complete frame <= 5 ms p95
```

Expected work:

```text
one todo field update
completed counter delta
footer/count patches
toggle-all state patch if needed
one row patch
no list identity rebuild
```

### 13.3 TodoMVC toggle all

Target:

```text
100 todos loaded
click toggle_all
source event -> complete rerender ideally < 10 ms
```

Required lowering:

```text
one event
one turn snapshot
one bulk loop over todos
one aggregate update
one patch batch
no 100 independent dynamic reactions
no per-item channel deliveries
no source rebinding
```

Trace should show:

```json
{
  "events_processed": 1,
  "todo_rows_touched": 100,
  "list_structure_rebuilds": 0,
  "source_rebindings": 0,
  "patch_batches": 1
}
```

### 13.4 Cells

For 26×100 cells:

```text
initial render <= 16 ms after warmup
edit plain cell <= 8 ms p95
edit formula dependency cell <= 10 ms p95
scroll/move focus <= 8 ms p95
only dirty/dependent visible cells rerendered
```

Required behavior:

```text
editing A1 dirties A1 and formulas depending on A1
=sum(A1:A3) updates when A1/A2/A3 changes
invalid formulas show deterministic errors
cycles show deterministic errors
no full-grid recompute for ordinary cell edit
```

---

## 14. Example verification scenarios

### 14.1 `counter`

Verify:

- initial value is 0,
- one increment produces 1,
- ten increments produce 10,
- state and rendered text match,
- source inventory contains increment button press.

### 14.2 `counter_hold`

Verify:

- same as counter,
- `HOLD state` uses previous committed value,
- multiple events in deterministic sequence produce exact expected count.

### 14.3 `interval`

Use fake clock.

Verify:

- initial value,
- after advancing fake time by one tick, rendered state changes exactly once,
- after advancing N ticks, count/value is exact,
- no real sleeping in tests.

### 14.4 `interval_hold`

Use fake clock.

Verify:

- same as interval,
- held state accumulates correctly,
- coalesced frame rendering does not corrupt logical tick count.

### 14.5 `todo_mvc`

Verify:

- initial todos render,
- add todo by typing + Enter,
- empty/whitespace todo is ignored,
- edit title,
- cancel edit with Escape,
- commit edit with Enter,
- blur behavior,
- toggle one todo,
- toggle all 100 todos under budget,
- clear completed,
- filters all/active/completed,
- remove one item,
- stale event from removed item is ignored/error in debug test mode,
- source families remain correct after append/remove/filter.

### 14.6 `todo_mvc_physical`

Same functional scenarios as `todo_mvc`.

Additional verification:

- physical/depth/debug layout is stable,
- hover/focus states update,
- checkbox/button source bindings still correct,
- text/title changes do not rebuild unrelated geometry,
- generated GPU and Ratatui debug projection agree semantically.

### 14.7 `cells`

Verify:

- grid headers A-Z and rows 1-100,
- visible grid cells render nicely in all backends,
- double-click enters edit mode,
- typing formula updates editor source state,
- Enter commits,
- Escape cancels,
- blur behavior matches intended example semantics,
- `=add(A1, A2)` updates when A1/A2 change,
- `=sum(A1:A3)` updates when A1/A2/A3 change,
- invalid formula deterministic error,
- cycle deterministic error,
- only dirty/dependent cells rerender.

### 14.8 `pong`

Use deterministic fake time and seeded/no-random input.

Verify:

- initial state,
- paddle movement from key holds,
- ball position after N frames,
- collisions invert velocity correctly,
- score/lives update deterministically,
- Ratatui grid and GPU frame agree semantically.

### 14.9 `arkanoid`

Use deterministic fake time and seeded/no-random input.

Verify:

- initial bricks,
- paddle movement,
- ball collisions,
- brick removal,
- score/lives,
- level reset/end state,
- Ratatui grid and GPU frame agree semantically.

---

## 15. Implementation phases

### Phase 1: Workspace and minimal compiler skeleton

Deliverables:

- workspace crates,
- parser skeleton,
- span diagnostics,
- basic Boon AST,
- nested module path parsing,
- source marker parsing,
- initial example file loading,
- `cargo check --workspace` passes.

Success criteria:

- can parse `counter`, `counter_hold`, `interval`, `interval_hold` snippets,
- can print AST/HIR diagnostics.

### Phase 2: Structural shapes and strict SOURCE binding

Deliverables:

- structural shape engine,
- host schema registry,
- source inventory builder,
- strict source binding errors,
- debug manifest output.

Success criteria:

- valid button/text_input examples pass,
- unbound/missing/unknown/conflicting sources fail with useful diagnostics,
- no nominal source constructors or source types are introduced.

### Phase 3: Deterministic runtime and counter codegen

Deliverables:

- `boon_runtime` turn machine primitives,
- generated Rust app for `counter`,
- generated Rust app for `counter_hold`,
- state inspection API,
- render patch emission.

Success criteria:

- counter and counter_hold pass state tests,
- no async/channels in runtime core,
- event dispatch is direct.

### Phase 4: Ratatui backend and first tests

Deliverables:

- HostViewIR basics,
- Ratatui backend,
- buffer snapshot tests,
- PTY test harness with `portable-pty`,
- counter/counter_hold Ratatui tests.

Success criteria:

- `cargo xtask verify ratatui` passes for counter examples,
- `cargo xtask verify ratatui --pty` passes for counter examples.

### Phase 5: Time examples

Deliverables:

- fake clock source,
- interval example,
- interval_hold example,
- deterministic time verification.

Success criteria:

- interval tests do not sleep,
- fake time advancement produces exact expected output.

### Phase 6: Owned dynamic lists and TodoMVC

Deliverables:

- source families,
- stable item IDs/generations,
- keyed list arena,
- List/append and List/remove lowering,
- TodoMVC example source migration to `SOURCE`,
- TodoMVC Ratatui backend,
- stale dynamic source tests.

Success criteria:

- add/remove/toggle/edit/filter/clear completed all pass in Ratatui,
- no dynamic stream subscription graph,
- stale event after remove is caught/ignored.

### Phase 7: TodoMVC performance and bulk optimization

Deliverables:

- aggregate lowering for count/retain/count,
- toggle_all bulk lowering,
- keyed visible projection,
- performance trace output.

Success criteria:

- 100-todo toggle all under target on local perf machine,
- checking one todo extremely fast,
- typing input has no visible lag and passes p95/p99 budgets.

### Phase 8: Cells dense grid

Deliverables:

- static range × range detection,
- dense cell IDs,
- cell source families,
- formula parser/evaluator or integration with existing example formula library,
- dependency graph/dirty recomputation,
- Ratatui grid rendering.

Success criteria:

- 26×100 grid renders nicely in Ratatui,
- formula dependencies update correctly,
- dirty recompute tests pass,
- no full-grid recompute for ordinary edit.

### Phase 9: WESL/wgsl_bindgen/wgpu renderer core

Deliverables:

- WESL shader roots,
- build script or xtask shader generation,
- `wgsl_bindgen` generated bindings,
- wgpu offscreen renderer,
- framebuffer readback,
- basic UI rectangles/text/grid rendering.

Success criteria:

- `cargo xtask shaders` generates WGSL/Rust bindings,
- headless wgpu frame render works,
- no winit/Sokol/SDL3/Slang dependency.

### Phase 10: Native app_window backend

Deliverables:

- app_window host,
- native wgpu surface integration,
- event loop integration,
- source input mapping,
- internal framebuffer capture before present.

Success criteria:

- all simple examples run natively,
- native headless tests pass,
- app_window smoke tests pass,
- no winit fallback added.

### Phase 11: Browser wgpu backend

Deliverables:

- wasm/browser runner,
- browser wgpu initialization,
- test-only `window.__boonTest`,
- Playwright tests for Chromium and Firefox,
- browser frame hash/readback.

Success criteria:

- counter/todo/cells run in browser,
- Playwright tests pass in Chromium and Firefox,
- browser timing traces generated.

### Phase 12: Physical TodoMVC, pong, arkanoid

Deliverables:

- physical/debug rendering path,
- Ratatui projection for physical UI,
- GPU physical/debug draw path,
- pong and arkanoid deterministic examples,
- scenarios across all backends.

Success criteria:

- all examples run across Ratatui/native/browser,
- all verification scenarios pass.

### Phase 13: CI and quality gates

Deliverables:

- CI workflow,
- cargo fmt/clippy/test,
- shader generation check,
- Ratatui buffer tests,
- optional PTY tests,
- native headless wgpu tests where CI GPU is available,
- browser Chromium/Firefox tests where CI supports them,
- artifact upload for failures.

Success criteria:

- failures include readable traces, frame artifacts, source inventory, and state snapshots.

---

## 16. Diagnostics and manifests

Every compiled app should produce a manifest useful for debugging.

Include:

```text
sources:
  static source paths
  dynamic source families
  inferred structural shapes
  host producer/binder
  readers

state:
  state cells
  HOLD cells
  derived values
  list owners/items

render:
  node IDs
  source bindings
  keyed list projections

performance:
  dirty nodes
  patch counts
  event counts
```

Example source inventory:

```text
#0 store.sources.new_todo_input.text
   shape: TEXT
   producer: Element/text_input(element.text)
   readers: store.title_to_add, new_todo_title_text_input.text

#1 store.sources.new_todo_input.event.key_down.key
   shape: tags { Enter, Escape, Backspace, Character, ... }
   producer: Element/text_input(element.event.key_down.key)
   readers: store.title_to_add

#10 store.todos[*].sources.checkbox.event.click
   shape: []
   owner: store.todos item
   producer: Element/checkbox(element.event.click)
   readers: new_todo.completed.HOLD
```

---

## 17. Snapshot and replay

Support deterministic replay:

```text
compiled app manifest
+ initial snapshot
+ source event log
= exact final state and render patch sequence
```

Snapshot should include:

- app state,
- held state,
- dynamic list item data,
- source state values if meaningful (`text`, `hovered`, `checked`, etc.),
- fake clock time in tests,
- generation counters.

Snapshot should not include:

- Rust futures,
- wakers,
- channel internals,
- OS/window handles,
- GPU resources.

---

## 18. Codex implementation rules

When implementing:

1. Do not add user-facing language features unless this file is explicitly updated.
2. Prefer structural validation and compile errors.
3. Keep Boon source pure data.
4. Do not introduce source constructors or nominal source types.
5. Do not call button press a `Pulse` in user-facing APIs; internally use `EmptyRecord` / `[]` shape.
6. Do not call `True`/`False` a user-facing `Bool`; internally use tag sets.
7. Do not merge unrelated event/state payloads; `key_down.key` is key only, text lives at `.text`.
8. Do not model Boon streams with Rust async/channels.
9. Keep rendering backend-independent through patches/IR.
10. Use Ratatui for first correctness path.
11. Use `app_window`, not winit.
12. Use WESL and `wgsl_bindgen`; do not add Slang initially.
13. Use framebuffer/readback/test APIs for visual verification, not OS screenshots.
14. Run tests after each meaningful implementation step.
15. Preserve readable diagnostics and manifests.
16. If a hard design choice is ambiguous, choose the simpler deterministic option and document it.

---

## 19. References for implementation agents

These references explain external crates/tools used by this plan:

- Ratatui `TestBackend` / terminal testing: <https://docs.rs/ratatui/latest/ratatui/backend/struct.TestBackend.html>
- Ratatui snapshot testing guide: <https://ratatui.rs/recipes/testing/snapshots/>
- portable-pty: <https://docs.rs/portable-pty>
- app_window: <https://docs.rs/app_window>
- wgpu: <https://docs.rs/wgpu>
- WESL Rust crate: <https://docs.rs/wesl/>
- WESL Rust getting started: <https://wesl-lang.dev/docs/Getting-Started-Rust>
- wgsl_bindgen: <https://docs.rs/wgsl_bindgen>
- Playwright browsers: <https://playwright.dev/docs/browsers>

