# Boon Rust Implementation Plan

This file is the implementation brief for the new `boon-rust` repository. It is written for Codex CLI / AI implementation agents and should be treated as the source of truth unless a human maintainer explicitly updates it.

The goal is a simple, strict, deterministic, fast Boon-to-Rust compiler/runtime with three verified rendering targets:

1. **Ratatui** for terminal apps, CI, AI-debuggable snapshots, and PTY tests.
2. **Native WebGPU/wgpu** using `app_window`, WESL, `wgsl_bindgen`, and framebuffer readback verification.
3. **Browser WebGPU/wgpu** using the same generated shaders and Firefox-first verification through a checked-in WebExtension harness.

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

All examples must eventually run and be automatically verified on Ratatui, native wgpu, and Firefox browser wgpu. Do not add Chromium tests or Chromium harnesses unless this plan is explicitly updated.

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
- A source record passed to a host function contains a `SOURCE` leaf that is not bindable by that host function.
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
- browser test driver,
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

Preserve these crate names, but keep early internals simple. It is acceptable for
some crates to start as thin wrappers over internal modules while examples are
being brought up. Do not spend early phases building elaborate cross-crate
abstractions before the target examples compile and run.

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

Do not simplify the language by dropping constructs required by the maintained `examples/<name>/source.bn` programs.

### `boon_hir`

Typed/validated high-level IR, symbol table, module/file graph, resolved names, lowered `PASS` / `PASSED` environments.

This should preserve enough source structure for diagnostics.

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
If keys later need payloads, use structural tagged variants such as
`Character [text: TEXT]`, not a nominal user-facing `Key` type.

`Unknown` must not survive final validation. Any reachable expression or source
with unknown shape is a compile error.

`Union` is allowed only when all possible shapes are explicitly known and
validated. It must not be used to hide incompatible branches.

### `boon_host_schema`

Structural contracts for runtime/host functions.

All source leaves accepted by element host functions are optional from the element's perspective.
A host contract declares which source leaves are bindable if present. It does not require an
element call to supply every possible source leaf.

Examples:

```text
Element/button(element)
  element.event.press -> EmptyRecord
  element.hovered     -> TagSet { False, True } optional
  element.focused     -> TagSet { False, True } optional

Element/text_input(element)
  element.text               -> Text optional
  element.event.change       -> EmptyRecord optional
  element.event.key_down.key -> Key tags optional
  element.event.blur         -> EmptyRecord optional
  element.event.focus        -> EmptyRecord optional

Element/checkbox(element)
  element.event.click -> EmptyRecord optional
  element.checked     -> TagSet { False, True } optional
  element.hovered     -> TagSet { False, True } optional

Element/label(element)
  element.event.double_click -> EmptyRecord optional
  element.hovered            -> TagSet { False, True } optional
```

The exact contract is maintained in Rust data, not Boon nominal source declarations.

If an element accepts a source record, every `SOURCE` leaf in that record must be accounted for by that host function's source contract. Unknown source fields are errors.
If app logic reads a source path, that path must still be declared and proven to have exactly one live producer.

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
- expose test-only JS API for browser harnesses,
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
cargo xtask verify browser-wgpu --browser firefox
cargo xtask bench todo_mvc --backend all --todos 100
cargo xtask bench cells --backend all --rows 100 --cols 26
cargo xtask bootstrap
cargo xtask bootstrap --check
cargo xtask shaders
cargo xtask generate
cargo xtask examples list
cargo xtask doctor firefox-webgpu
cargo xtask firefox install-native-host
cargo xtask firefox reset-profile
```

Initial dependency pins:

```text
app_window = 0.3.3
glyphon = 0.11.0
slotmap = 1.1.1
wesl = 0.3.2
wgpu = 29.0.1
wgsl_bindgen = 0.22.2
web-ext npm package = 10.1.0
```

Initial toolchain facts for this machine:

```text
rustc = 1.95.0
cargo = 1.95.0
node = 22.22.0
npm = 10.9.4
firefox = 149.0
```

### Tool bootstrap policy

Unattended verification must install missing tools instead of stopping for manual
setup whenever installation can be done safely from the repository.

Implement:

```bash
cargo xtask bootstrap
cargo xtask bootstrap --check
```

`cargo xtask verify all` must run `cargo xtask bootstrap` first unless an explicit
`--no-bootstrap` flag is passed.

Bootstrap requirements:

- install Rust targets needed by the plan, including `wasm32-unknown-unknown`,
  by running `rustup target add ...` when `rustup` is available,
- install repo-local npm tools under `.boon-local/tools/`; do not require or use
  global npm packages,
- install `web-ext@10.1.0` with `npm --prefix .boon-local/tools install web-ext@10.1.0`
  if the repo-local copy is missing or has the wrong version,
- invoke `.boon-local/tools/node_modules/.bin/web-ext` from xtask commands,
- create/update `.boon-local/firefox-profile/user.js`,
- build the Firefox native messaging host,
- install or refresh the Firefox native messaging manifest automatically when it
  is missing or stale,
- create all required `.boon-local/` directories idempotently.

If a missing prerequisite requires system package manager privileges, such as
installing stable Firefox or GPU drivers, bootstrap must attempt the supported
platform installer when it can do so non-interactively. If privileges or platform
support are unavailable, it must fail before tests start with the exact command
the maintainer can run. Do not skip browser or GPU gates silently.

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

### 2.2.1 Source batches and source-state ordering

A host/backend may deliver a `SourceBatch`, not just one isolated source emission.

A `SourceBatch` contains:

- zero or more source state updates, e.g. `input.text = TEXT { abc }`, `button.hovered = True`,
- zero or more source event emissions, e.g. `input.event.change = []`, `input.event.key_down.key = Enter`.

For each batch:

1. Validate all source paths and owner generations.
2. Apply source state updates to the app's source-state table.
3. Dispatch event emissions in deterministic order.
4. Each event emission creates one Boon turn.
5. `THEN`/`WHEN` bodies sample source state after step 2 and after previous turns in the same batch have committed.

This keeps `key_down.key` separate from `.text`, while still allowing code to sample
current text from source state:

```boon
source.event.key_down.key
|> WHEN {
    Enter => source.text |> Text/trim()
    __ => SKIP
}
```

The key event carries only a key tag. The text value is sampled from the source state.

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
For element functions, every source leaf is optional. The contract is a list of bindable
leaves and their shapes, not a list of leaves that every call must provide.

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
    .optional("text", Shape::Text)
    .optional("event.change", Shape::EmptyRecord)
    .optional("event.key_down.key", Shape::key_tags())
    .optional("event.blur", Shape::EmptyRecord)
    .optional("event.focus", Shape::EmptyRecord)
    .optional("focused", Shape::tag_set(["False", "True"]));
```

For checkbox:

```rust
HostContract::new("Element/checkbox")
    .source_arg("element")
    .optional("event.click", Shape::EmptyRecord)
    .optional("checked", Shape::tag_set(["False", "True"]))
    .optional("hovered", Shape::tag_set(["False", "True"]));
```

If a host function does not produce a source path, the compiler must not assume it exists.

### 3.1.1 Conditional and lifecycle source binding

A `SOURCE` leaf must have exactly one statically provable binding site or binding
family.

At runtime, a binding may be conditionally live if the host element is under
`WHEN`/`WHILE`/list rendering. This is allowed only if:

- every live runtime state has at most one live producer for the source leaf,
- the compiler can prove the binding site/family exists,
- the backend refuses source events from non-live or stale producers in test/debug builds,
- stale dynamic owner generations are rejected.

So `edit_input.event.change` inside an editing branch is valid even when the edit
input is not currently rendered; it simply cannot produce events while unmounted.

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

### 4.1.1 User-facing list operation semantics

The compiler may optimize list operations, but it must preserve these source-level meanings:

- `List/append(item: expr)` appends one item whenever `expr` emits a non-`SKIP` value.
- `List/remove(item, on: expr)` removes the current item whenever `expr` emits a non-`SKIP` value for that item.
- `List/retain(item, if: predicate)` is a derived immutable view. Do not use it as a source-of-truth mutation operation.
- `List/map(item, new: expr)` is a derived keyed projection when the input list has stable item identity.
- `List/count()` over a stable list returns the current item count and may be maintained incrementally.
- `SKIP` means no emission/no operation; it is not a value stored in the list.

All optimized lowerings must be checked against the reference interpreter on the
same input event log.

### 4.2 Owned list representation

Dynamic lists should not become runtime stream graphs.

Use owned arenas/keyed vectors. Use `slotmap = 1.1.1` for generational owner
identity; do not implement a custom arena and do not depend on an unspecified
`SlotVec` type.

```rust
struct TodoList {
    items: SlotMap<TodoId, Todo>,
    order: Vec<TodoId>,
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
- deterministic windowed viewport.

Viewport rules:

- the logical grid is 26 columns by 100 rows,
- the initial viewport shows column headers, row numbers, and at least cells A1:C3,
- row 100 and column Z are reachable through deterministic scenario actions,
- viewport movement must not rebuild unrelated cell/source identity.

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

### 7.1.1 Initial mount and stable render identity

The generated app must emit an initial mount patch batch before handling user input.

Node identity rules:

- static render nodes get deterministic generated `NodeId`s,
- dynamic list item render nodes are keyed by owner identity plus stable local path,
- moving/filtering list items must preserve keyed node identity,
- removing an owner drops its render subtree and source bindings,
- reusing a removed owner's stale source generation is invalid.

A list filter change should produce keyed moves/inserts/removes, not full teardown
unless the source semantics require it.

### 7.1.2 Controlled input source-state synchronization

For host elements with source state such as `element.text`, the backend must keep
the source state and rendered widget state coherent.

When the user edits an input:

```text
host input value changes
  -> SourceBatch updates `element.text`
  -> SourceBatch emits `element.event.change = []`
```

When Boon renders a new text value for the same input:

```text
Boon emits SetText/SetInputText patch
  -> backend updates displayed widget text
  -> backend updates the associated source-state slot before the next source batch
```

This prevents stale reads after code such as TodoMVC clearing the new-todo input
after Enter.

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

### 7.3 Core app and backend contracts

The Boon core/app boundary is synchronous and exact:

```rust
pub trait BoonApp {
    fn mount(&mut self) -> TurnResult;
    fn dispatch_batch(&mut self, batch: SourceBatch) -> Vec<TurnResult>;
    fn snapshot(&self) -> AppSnapshot;
    fn source_inventory(&self) -> SourceInventory;
}

pub struct TurnResult {
    pub turn_id: TurnId,
    pub patches: Vec<HostPatch>,
    pub state_delta: StateDelta,
    pub metrics: TurnMetrics,
}
```

Input adapters translate backend/browser/terminal input into `SourceBatch`. Renderers
apply `HostPatch` and never own Boon semantics.

Renderer contracts:

```rust
pub trait RenderBackend {
    fn apply_patches(&mut self, patches: &[HostPatch]) -> anyhow::Result<()>;
    fn render_frame(&mut self) -> anyhow::Result<FrameInfo>;
    fn capture_frame(&mut self) -> anyhow::Result<FrameSnapshot>;
    fn metrics(&self) -> BackendMetrics;
}

pub trait InputAdapter {
    fn translate_input(&mut self, input: BackendInput) -> anyhow::Result<Option<SourceBatch>>;
}
```

Host async/event-loop code may wrap these traits, but no async/task/channel model may
enter the generated Boon app or `boon_runtime`.

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

The native playground GUI is governed by the companion plan
`docs/plans/native_gui_playground_plan.md`. That plan is part of the source of
truth for Phase 10 and all verification prompts. A text-only app_window surface
or terminal transcript rendered into a window is not an acceptable native
playground.

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

Native tests must support a headless/offscreen mode:

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
  also read back the actual app_window/wgpu surface texture before present
  verify nonblank/color diversity and final live surface size == render size
```

The app_window gate must not be satisfied by an offscreen/internal framebuffer
alone. It must launch the real app_window RGBA path for every maintained example
in an isolated helper process, record `visible-surface-frame.json`, and fail on
blank/solid frames or stale configured size after the compositor has resized the
surface. OS compositor screenshots may be recorded as diagnostic artifacts when
a local screenshot backend is available, but deterministic pass/fail remains the
app_window surface readback plus semantic/state/hash gates.

Native app_window verification must also drive the native playground shell with
app_window-shaped mouse/key samples for every maintained example and write
`playground-interactions.json`. These scenarios must include sidebar/example
selection, visible-control hit testing, typed text where applicable, keyboard
controls for games, live interval/game advancement, and state/frame assertions.
TodoMVC must have multiple playground scenarios covering add, whitespace reject,
edit, remove, checkbox toggle, filters, clear completed, and outside-click
non-mutation.

### 9.4 GPU text rendering

Text rendering is required early because TodoMVC and Cells depend on it.

Use `glyphon` for v1 GPU text rendering.

Do not start by hand-writing a complex SDF/MSDF text renderer or custom glyph atlas
unless the plan is explicitly updated.

Text rendering must support:

- ASCII at minimum for the target examples,
- stable layout metrics for tests,
- cell/grid text clipping,
- deterministic frame output for verification.

---

## 10. Browser wgpu backend

Browser GPU rendering uses:

- Rust wasm,
- wgpu browser/WebGPU backend,
- same WESL-generated WGSL roots,
- same `wgsl_bindgen` generated Rust module, compiled with target-specific `cfg` only where unavoidable,
- Firefox verification through a checked-in WebExtension harness.

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

### 10.2 Firefox WebExtension verification

Do not use Playwright for browser v1. Drive Firefox through a checked-in
WebExtension harness and native messaging.

Repository layout:

```text
crates/boon_verify/firefox_extension/
  manifest.json
  background.js
  content.js
  page_bridge.js
```

The extension must have a fixed Gecko ID:

```json
{
  "browser_specific_settings": {
    "gecko": {
      "id": "boon-rust-test@boonlang.local"
    }
  }
}
```

The Firefox harness flow is:

```text
xtask starts local wasm test server
xtask launches Firefox with isolated repo-local profile
Firefox loads the checked-in WebExtension
background script connects to boon-firefox-native-host with native messaging
content script attaches only to the local test server origin
page bridge talks to window.__boonTest with window.postMessage
native host receives state, source inventory, frame hashes/frames, metrics, and failures
xtask stores artifacts
```

Native messaging requirements:

- build a Cargo native host binary, e.g. `boon-firefox-native-host`,
- generate the Firefox native messaging manifest from repo state,
- allow only `boon-rust-test@boonlang.local`,
- use Firefox's native JSON-over-stdio message framing,
- make `cargo xtask firefox install-native-host` install/update the manifest in the Firefox-required location,
- make `cargo xtask doctor firefox-webgpu` detect missing or stale native host
  manifests and repair them by invoking the same installer path used by
  `cargo xtask firefox install-native-host`.

Use an isolated Firefox profile:

```text
.boon-local/firefox-profile/
```

`xtask` must always launch Firefox with this profile and must refuse to use the user's normal profile.
Before launch, `xtask` must create or update `.boon-local/firefox-profile/user.js`
with:

```js
user_pref("dom.webgpu.enabled", true);
```

Use the repo-local `web-ext` executable from `.boon-local/tools/node_modules/.bin/`
with `run --firefox-profile <repo>/.boon-local/firefox-profile --keep-profile-changes`
so the extension/profile state remains stable between test passes.
`cargo xtask firefox reset-profile` may delete only `.boon-local/firefox-profile/`.

### 10.3 Browser verification

Browser tests must:

```text
serve wasm app
open page in Firefox isolated profile first
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
target/boon-artifacts/<example>/browser-firefox-extension/
  frame_000.png
  trace.json
  timings.json
  extension.log
  native-host.log
```

### 10.4 Browser WebGPU platform gates

Browser GPU tests must require real WebGPU (`navigator.gpu`) for the WebGPU
backend. Do not silently accept a WebGL2 fallback for WebGPU verification.

Stable Firefox is the browser v1 target. `cargo xtask doctor firefox-webgpu` must
launch the isolated profile with stable Firefox first and prove:

- `dom.webgpu.enabled` is true for that profile,
- `navigator.gpu` exists,
- adapter request succeeds,
- device request succeeds,
- the WebExtension loads,
- native messaging connects to `boon-firefox-native-host`.

If CI or the local machine lacks Firefox WebGPU support, the browser gate fails
with an explicit Firefox WebGPU capability error. Do not use Firefox Nightly,
Chromium, WebGL2, or any other browser/backend as a substitute.

All browser tests must feature-detect `navigator.gpu` and record the browser,
version, platform, and WebGPU adapter/device result in artifacts.

---

## 11. Shader pipeline

Use:

```text
WESL source
  -> build.rs / xtask compiles WESL to WGSL
  -> wgsl_bindgen generates Rust bindings
  -> wgpu renderer uses generated modules
```

WESL must be fully resolved to plain WGSL before `wgsl_bindgen` runs:

```text
.wesl roots
  -> WESL compiler/linker
  -> generated .wgsl files with no unresolved imports
  -> wgsl_bindgen
  -> generated Rust
```

Do not rely on `wgsl_bindgen` to understand WESL imports. Feed it WGSL.

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

Shader layout:

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

`cargo xtask shaders` must:

1. Compile WESL roots to generated WGSL files in `OUT_DIR` or `target/generated-shaders`.
2. Run `wgsl_bindgen` on the generated WGSL roots.
3. Generate one Rust module included by `boon_backend_wgpu`.
4. Fail fast on shader validation errors.

Pin versions of `wesl`, `wgsl_bindgen`, and `wgpu` in `Cargo.toml`/`Cargo.lock`.
Regenerate shader bindings only through `cargo xtask shaders`, never by hand.
`build.rs` may include generated bindings or fail if they are stale, but it must not
be a second shader-generation path with different behavior.

---

## 12. Verification framework

### 12.1 One scenario DSL for all platforms

Do not write separate test logic for each backend.

Use one scenario format: a Rust builder API in `boon_verify`. Do not add RON,
YAML, or JSON scenario fixtures for v1. JSON is used only for reports, traces,
manifests, and artifacts.

Example:

```rust
Scenario::new("todo_mvc_add_toggle_clear")
    .example("todo_mvc")
    .viewport(120, 40)
    .seed(1)
    .expect_text("What needs to be done?")
    .type_text("store.sources.new_todo_input", "Buy milk")
    .press_key("store.sources.new_todo_input.event.key_down.key", KeyTag::Enter)
    .run_until_idle()
    .expect_text("Buy milk")
    .expect_state("store.todos_count", InspectValue::number(3))
    .click("store.todos[0].sources.checkbox.event.click")
    .run_until_idle()
    .expect_text("2 items left")
    .click("store.sources.toggle_all_checkbox.event.click")
    .run_until_idle()
    .expect_state("store.completed_todos_count", InspectValue::number(3))
    .expect_frame_budget_ms(10.0);
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
cargo xtask verify browser-wgpu --browser firefox
cargo xtask bench todo_mvc --backend all --todos 100
cargo xtask bench cells --backend all --rows 100 --cols 26
cargo xtask bootstrap
cargo xtask bootstrap --check
cargo xtask examples list
cargo xtask doctor firefox-webgpu
cargo xtask firefox install-native-host
cargo xtask firefox reset-profile
```

### 12.4 Definition of successful implementation

The implementation is successful when the current development checkout can run:

```bash
cargo xtask verify all
```

with no manual setup beyond having Rust/cargo available. The command must
bootstrap repo-local tools, generate code/shaders, run all hard gates, and write:

```text
target/boon-artifacts/success.json
```

The success report must include:

- OS/platform,
- Rust/cargo versions,
- Firefox version and profile path,
- WebGPU adapter/device metadata for native and browser backends,
- exact command list executed by `verify all`,
- all scenario pass/fail results,
- all timing summaries,
- all frame artifact paths and hashes.

No example-specific success may be claimed from semantic state alone. TodoMVC and
Cells must pass deterministic state, source inventory, timing, replay, and frame
checks in Ratatui buffer, native headless wgpu, and Firefox WebGPU. The same
scenarios must also pass Ratatui PTY and native app_window functional/frame
smoke gates.

### 12.5 Deterministic frame and screenshot checks

Use internal render output, not OS screenshots, as the strict visual oracle.

Canonical capture rules:

- Ratatui captures canonical text buffers at `120x40`.
- GPU/browser captures internal RGBA frame textures at `1280x720`,
  device-pixel-ratio `1.0`, fixed scale, and no live animation during capture.
- Browser captures use `window.__boonTest.captureFrameRgba()`.
- Native captures use the owned offscreen frame texture before present.
- Native app_window captures additionally read back the real app_window surface
  texture before present, record the final live surface size, and require it to
  match the rendered frame size. This gate exists specifically to catch black
  visible windows, stale Wayland configure sizes, and text-only/window-layout
  regressions that internal offscreen frames alone can miss.
- Native app_window also records `playground-interactions.json` for each
  maintained example. These are not semantic shortcuts: they pass through the
  same native playground hit-testing/input handlers used by manual app_window
  runs, then assert semantic state and frame hashes after each step.
- All GPU/browser text uses one checked-in deterministic UI font; do not use
  system font fallback in tests.
- Each checked frame writes PNG artifact, raw RGBA hash, and semantic view-tree
  probes.
- Golden frame hashes are exact for the local hard gate and are keyed by
  example, scenario, backend, viewport, renderer version, font hash, and platform
  metadata.
- A changed frame hash is a test failure until the maintainer intentionally
  updates the expected artifact.

Required TodoMVC frame checkpoints:

- default initial data is exactly two active todos: `Buy groceries` and `Clean room`,
- 100-todo scenarios create exactly `Todo 001` through `Todo 100` by deterministic
  scenario actions before measuring,
- initial state with the maintained seed todos,
- after adding `Buy milk` with typing plus Enter,
- after rejecting whitespace-only input,
- after editing one todo title to `Buy oat milk`,
- after toggling one todo,
- after toggling all 100 todos,
- after filtering active,
- after filtering completed,
- after clearing completed.

Required Cells frame checkpoints:

- initial viewport showing headers, row numbers, and at least A1:C3,
- editing A1 before commit,
- after committing A1=`1`, A2=`2`, A3=`3`,
- after committing B1=`=add(A1, A2)`,
- after committing B2=`=sum(A1:A3)`,
- after changing A2 and observing dirty dependent updates,
- invalid formula error state,
- cycle error state,
- viewport moved to include row 100 and column Z.

---

## 13. Performance targets

All timing gates use source-event-to-complete-frame:

```text
source event received
  -> Boon state update
  -> dirty recomputation
  -> render patch generation
  -> backend patch application
  -> final terminal buffer or GPU frame texture ready
```

Performance budgets measure source-event-to-frame-ready, not artifact readback.

For GPU backends:

- include Boon turn time,
- include patch application,
- include command encoding/submission needed to make the frame ready,
- exclude PNG encoding,
- exclude framebuffer readback used only for verification artifacts,
- record readback time separately.

For app-window mode, record OS input-to-source latency when the host exposes it,
but do not mix compositor/vsync latency into the core compiler/runtime budget.

Benchmark rules:

- run 5 warmup iterations before measuring each scenario,
- run 30 measured iterations unless the scenario explicitly says otherwise,
- use fixed viewport sizes from Section 12.5,
- use deterministic seed `1`,
- record raw per-iteration timings in `timings.json`,
- fail the gate on budget misses instead of only reporting them.

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

This budget applies to Ratatui buffer, native headless wgpu, and Firefox WebGPU
internal frame-ready timings. Ratatui PTY and native app_window must record the
same metric when available and must at minimum pass the functional and frame
checks.

### 13.2 TodoMVC checking one item

Target:

```text
100 todos loaded
click one checkbox 30 times across deterministic item IDs
source event -> complete frame <= 5 ms p95
p99 <= 10 ms
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
source event -> complete frame <= 10 ms p95
p99 <= 16 ms
max <= 25 ms
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

Detailed Cells budgets:

```text
mount to first frame after shader/font warmup <= 16 ms p95
edit A1 plain value over 30 measured iterations <= 8 ms p95, <= 16 ms p99
edit A2 while B1 =add(A1, A2) and B2 =sum(A1:A3) depend on it <= 10 ms p95, <= 16 ms p99
move selection within visible viewport 30 times <= 8 ms p95, <= 16 ms p99
move viewport to row 100 / column Z <= 10 ms p95, <= 20 ms p99
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

### Phase 0: Golden examples and compatibility fixtures

Deliverables:

- write maintained runnable Boon programs for these targets:
  - `counter`
  - `counter_hold`
  - `interval`
  - `interval_hold`
  - `todo_mvc`
  - `todo_mvc_physical`
  - `cells`
  - `pong`
  - `arkanoid`
- write the maintained runnable Boon program as `examples/<name>/source.bn`,
- store maintained expectations as `examples/<name>/expected.*`,
- write clean maintained pure-data `SOURCE` Boon; behavior must satisfy `expected.*`,
- use `/home/martinkavik/repos/boon`, `/home/martinkavik/repos/boon-zig`, and `/home/martinkavik/repos/boon-pony` only as inspiration while writing the maintained `source.bn` files,
- do not copy or preserve legacy example files or legacy example directories in this repo,
- keep all example business logic in Boon; do not add TodoMVC, Cells, Pong, or Arkanoid logic to runtime or stdlib,
- add parser fixture tests that snapshot AST/HIR for each `source.bn`,
- add source inventory snapshots for each `source.bn`,
- add a rule that the compiler must not silently change Boon semantics to make examples easier.

Success criteria:

- all target examples have `examples/<name>/source.bn`,
- `cargo xtask examples list` shows them,
- parser tests fail if example syntax is not supported,
- source inventory snapshots fail if SOURCE migration changes accidentally.

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
- clean Boon implementation of 7GUIs Cells behavior in `examples/cells/source.bn`,
- dependency graph/dirty recomputation,
- Ratatui grid rendering.

Success criteria:

- 26×100 grid renders nicely in Ratatui,
- formula dependencies update correctly for the 7GUIs behavior, including numeric cells, references, `=add(A1, A2)`, and `=sum(A1:A3)`,
- dirty recompute tests pass,
- no full-grid recompute for ordinary edit,
- no Cells-specific formula parser, evaluator, or business rules are added to runtime or stdlib.

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
- internal framebuffer capture before present,
- graphical native playground shell and preview surface from
  `docs/plans/native_gui_playground_plan.md`.

Success criteria:

- all simple examples run natively,
- native headless tests pass,
- app_window smoke tests pass by launching the app_window host, creating a wgpu
  surface, translating one synthetic input to `SourceBatch`, dispatching through
  `BoonApp`, rendering one internal frame texture, capturing a nonblank frame hash,
  reading back the actual app_window surface texture with a live-size match proof,
  running native playground interaction scenarios for every maintained example,
  and exiting cleanly,
- `cargo xtask playground native --example todo_mvc` displays a graphical
  TodoMVC-like preview with actual visible input, checkbox, filter, row, and
  clear-completed regions,
- interval examples tick from live host time in manual native playground mode,
- `pong` and `arkanoid` advance automatically in manual native playground mode
  and accept keyboard control,
- no winit fallback added.

### Phase 11: Browser wgpu backend

Deliverables:

- wasm/browser runner,
- browser wgpu initialization,
- test-only `window.__boonTest`,
- Firefox WebExtension verification harness,
- native messaging host and manifest installer,
- isolated repo-local Firefox profile launcher,
- browser frame hash/readback.

Success criteria:

- `cargo xtask doctor firefox-webgpu` passes after bootstrap or fails only for an
  explicit system/browser/GPU capability that cannot be installed from the repo,
- `cargo xtask firefox install-native-host` installs or refreshes the native messaging manifest,
- counter/todo/cells run in browser,
- Firefox WebExtension tests pass,
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

### Phase 13: Local hard quality gates

Deliverables:

- unattended bootstrap through `cargo xtask bootstrap`,
- cargo fmt/clippy/test,
- shader generation check,
- Ratatui buffer tests,
- required PTY tests,
- native headless wgpu tests,
- native app_window smoke tests,
- Firefox WebExtension tests with isolated profile and stable Firefox WebGPU,
- TodoMVC and Cells timing budget gates,
- TodoMVC and Cells deterministic frame/hash gates,
- `target/boon-artifacts/success.json`,
- artifact upload for failures.

Success criteria:

- `cargo xtask verify all` bootstraps missing repo-local tools in the current
  development checkout and runs the Ratatui buffer, Ratatui PTY, native headless
  wgpu, native app_window, and Firefox WebGPU gates,
- TodoMVC and Cells pass the hard timing budgets and deterministic frame/hash
  checkpoints from Sections 12.4, 12.5, and 13,
- missing PTY support, missing stable Firefox WebGPU, or missing native app_window
  support is a hard local gate failure,
- failures include readable traces, frame artifacts, source inventory, state
  snapshots, and bootstrap/tool logs.

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
- state-like source values if meaningful (`text`, `hovered`, `focused`, `checked`, etc.),
- fake clock time in tests,
- generation counters.

Do not snapshot one-shot `[]` event emissions except in event logs.

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
13. Keep all element source leaves optional; host schemas define bindable leaves, not required leaves.
14. Verify browser WebGPU through the Firefox WebExtension/native-messaging harness for unattended v1.
15. `window.__boonTest` must be omitted from release builds and guarded by a test/dev feature flag.
16. Browser WebGPU tests must require `navigator.gpu`; missing Firefox WebGPU support is a hard browser-gate failure.
17. Use framebuffer/readback/test APIs for visual verification, not OS screenshots.
18. Exclude verification readback/PNG encoding from interactive performance budgets and record them separately.
19. Keep all example business logic in Boon; runtime and stdlib may contain only generic language/runtime primitives.
20. Use `glyphon` for v1 GPU text.
21. Use `slotmap` for dynamic owner storage.
22. Use Rust builder scenarios in `boon_verify`; JSON is only for reports/artifacts/manifests.
23. Run tests after each meaningful implementation step.
24. Preserve readable diagnostics and manifests.
25. Make `cargo xtask verify all` unattended: bootstrap missing repo-local tools
    automatically and fail early with exact system-install commands only when a
    prerequisite cannot be installed without unavailable system privileges.
26. Treat TodoMVC and Cells timing budgets plus deterministic frame/hash checks
    as required success gates, not optional benchmarks.
27. If a hard design choice is ambiguous, stop and update this plan instead of inventing implementation policy.

---

## 19. References for implementation agents

These references explain external crates/tools used by this plan:

- Ratatui `TestBackend` / terminal testing: <https://docs.rs/ratatui/latest/ratatui/backend/struct.TestBackend.html>
- Ratatui snapshot testing guide: <https://ratatui.rs/recipes/testing/snapshots/>
- portable-pty: <https://docs.rs/portable-pty>
- app_window 0.3.3: <https://docs.rs/app_window/0.3.3>
- glyphon 0.11.0: <https://docs.rs/glyphon/0.11.0>
- slotmap 1.1.1: <https://docs.rs/slotmap/1.1.1>
- wgpu 29.0.1: <https://docs.rs/wgpu/29.0.1>
- WESL Rust crate 0.3.2: <https://docs.rs/wesl/0.3.2>
- WESL Rust getting started: <https://wesl-lang.dev/docs/Getting-Started-Rust>
- wgsl_bindgen 0.22.2: <https://docs.rs/wgsl_bindgen/0.22.2>
- Firefox WebExtension content scripts: <https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Content_scripts>
- Firefox WebExtension native messaging: <https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Native_messaging>
- Firefox native manifests: <https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Native_manifests>
- web-ext npm package 10.1.0: <https://www.npmjs.com/package/web-ext/v/10.1.0>
- web-ext run and Firefox profiles: <https://extensionworkshop.com/documentation/develop/getting-started-with-web-ext/>
