# Pure Boon Stdlib Migration Plan

This document is the next stricter contract after
`docs/plans/generic_boon_language_plan.md`.

The current implementation has removed maintained-example name branches, but it
still has Rust semantic families for list/repeater apps, dense grids/formulas,
and dynamics/kinematics games. That is still too much Rust-side business logic.

The target state is:

```text
examples/<name>/source.bn
  -> Boon parser
  -> typed Boon HIR
  -> generic Boon executable IR
  -> generated Rust app code plus a small runtime/bridge
  -> Boon-built scene tree
  -> Rust generic renderer/backend
```

Rust may implement the Boon compiler, verifier, Rust code generator, a small
deterministic runtime bridge, standard library primitives, host SOURCE plumbing,
backend drawing, hit testing, input collection, timing, and screenshots. Rust
must not implement app semantics, app-specific view construction, list UI
behavior, spreadsheet logic, or game physics as runtime families.

## Non-Negotiable Rules

- Do not weaken existing hard gates from `IMPLEMENTATION_PLAN.md`,
  `docs/plans/boon_powered_migration_plan.md`,
  `docs/plans/native_gui_playground_plan.md`, or `prompter.json`.
- Do not change `cosmic-background-launch` command parameters in this repo until
  the external compositor/helper work is available after reboot.
- Do not add Chromium, Playwright, WebGL2 fallback, winit fallback, legacy
  example copies, nominal source constructors, async/channels inside the Boon
  runtime, `Sheet/new`, or app-specific TodoMVC/Cells/Pong/Arkanoid runtime
  logic.
- Keep example business logic in `examples/<name>/source.bn`.
- Rust code may contain names of maintained examples only in tests, fixtures,
  verification scenarios, artifact paths, and generated expected files.
- Verification scenarios may be example-aware, but must drive public
  SOURCE/input/render boundaries. They must not provide hidden business logic.

## Allowed Rust Responsibilities

Rust is allowed to implement these layers:

- parser, diagnostics, span/provenance tracking,
- type/shape checker,
- HIR and generic executable IR construction,
- Rust code generation from generic Boon IR,
- a small deterministic runtime bridge for SOURCE batches, state storage,
  identity, and backend IO,
- host SOURCE validation and batching,
- generic stdlib functions,
- generic scene tree renderer,
- backend plumbing for Ratatui, native app_window/wgpu, native headless wgpu,
  Firefox WebGPU/WebExtension,
- screenshot/frame hash capture,
- timing and replay verification.

Rust stdlib primitives must be value-level and reusable. Examples:

- `Number/add`, `Number/sub`, `Number/min`, `Number/max`, `Number/clamp`,
- `Text/trim`, `Text/is_not_empty`, `Text/from_number`,
- `Bool/not`, comparisons, predicates,
- `List/append`, `List/remove`, `List/filter`, `List/map`, `List/fold`,
  `List/count`, `List/range`,
- `Record/get`, `Record/set`,
- `Geometry/intersects`, `Geometry/contains`, `Geometry/reflect`,
- formula parsing and evaluation must be written in Boon source or in a future
  explicit Rust stdlib crate with a public, reusable library API; do not put a
  hidden Cells-specific formula engine in the runtime,
- scene primitives such as rectangles, text, buttons, text inputs, checkboxes,
  panels, grids, and layers.

Rust stdlib primitives must not remember or infer TodoMVC, Cells, Pong,
Arkanoid, counter, or interval structure.

## Forbidden Rust Responsibilities

Remove or replace these current-family concepts:

- list/repeater runtime family that owns TodoMVC-like state transitions,
- list UI renderer that knows item rows, filters, clear-completed layout, or
  item counts outside Boon-authored scene code,
- dense grid runtime family that owns spreadsheet selection, formula bar,
  dependency recomputation, or formula semantics outside Boon/stdlib,
- dynamics/kinematics runtime family that owns game frame progression,
  collision rules, paddle motion, brick fields, score/lives, or resets,
- hardcoded snapshot aliases such as `kinematics.*` that exist only because a
  Rust family owns the behavior,
- compiler lowering that recognizes top-level records like `kinematics` or
  `grid` as privileged app families,
- render dispatch such as `render_repeater_scene`, `render_matrix_scene`, or
  `render_dynamics_scene` where Rust constructs app-specific scene layout.

The final runtime may still have generic data structures such as lists, maps,
records, arrays, and scene nodes. Those must be driven by Boon code and generic
stdlib calls, not by recognized app families.

## Target Architecture

### Boon Source

Each example source file must describe:

- state declarations,
- SOURCE bindings,
- event handlers,
- derived values,
- list/grid/game state transitions,
- view/scene tree construction,
- frame/tick behavior where applicable.

For example:

- TodoMVC source owns todo creation, filtering, editing, toggling, clear
  completed, item count text, footer visibility, and row scene construction.
- Cells source owns selected cell state, formula bar state, dependency
  recomputation policy, visible grid scene construction, and formula evaluation
  calls.
- Pong and Arkanoid source own frame step, input handling, movement, collision,
  brick removal, scoring/lives/resets, and scene construction.

### Compiler

The compiler must transpile Boon into Rust through generic semantics:

- no `ProgramSpec` family recognition,
- no `IrListState`, `IrMatrixModel`, or `IrDynamicsModel` as semantic families,
- no top-level record names with privileged behavior,
- no branch that says "if this looks like a game/list/grid, build a special
  Rust runtime model".

Acceptable compiler output:

- functions,
- state slots,
- source bindings,
- event handlers,
- derived value graph,
- scene tree builders,
- generic executable IR operations,
- typed stdlib call nodes,
- generated Rust app modules that call only generic bridge and stdlib APIs.

### Runtime Bridge

The runtime should stay small. Prefer generated Rust app code over a full
generic VM. The bridge must provide only generic host integration:

- mount/dispatch entrypoints for generated Rust app modules,
- SourceBatch validation and host SOURCE plumbing,
- stable dynamic owner identity and generation checks,
- storage helpers for generated state values,
- dirty scheduling helpers if needed,
- generic scene patch delivery to renderers,
- frame/tick sources as normal host sources.

The runtime must not know whether it is running TodoMVC, Cells, Pong, Arkanoid,
counter, or interval.

### Renderer

The renderer receives a generic scene tree and draws it.

Allowed renderer knowledge:

- primitive type: rectangle, text, image, button, text input, checkbox, grid
  line, layer, clip, transform,
- colors, borders, font size, layout boxes,
- hit targets and source bindings attached to scene nodes,
- backend-specific GPU/text/shader details.

Forbidden renderer knowledge:

- "todo row",
- "filter button",
- "spreadsheet formula bar",
- "paddle",
- "ball",
- "brick",
- "counter button",
- "interval label".

If a backend needs batching or specialized drawing, that optimization must be
below the generic scene primitive layer.

## Required Verification

### Static Anti-Cheat Gate

Add or strengthen a gate that fails if implementation files contain maintained
example names or forbidden family structures outside allowed test/verification
locations.

The gate must reject:

- maintained example names in compiler/runtime/backend implementation files,
- `ProgramSpec` family recognition,
- semantic structs such as `IrListState`, `IrMatrixModel`, `IrDynamicsModel`,
  `MatrixRuntimeState`, `DynamicsRuntimeState`, and equivalents,
- functions that construct app-family scenes in Rust,
- Rust snapshot keys that exist only because an app-family runtime owns state.

The gate must allow:

- stdlib function names like `List/map` or `Geometry/intersects`,
- generic scene primitive names,
- example names in tests, fixtures, scenarios, expected artifact paths, and
  docs.

### Mutation Probes

Mutation probes must prove behavior depends on Boon source:

- changing TodoMVC filtering/toggle/clear source changes or fails behavior,
- changing Cells formula/dependency source changes or fails behavior,
- changing Pong/Arkanoid collision or movement source changes or fails behavior,
- removing scene construction in source changes frame hashes,
- deleting or renaming SOURCE bindings fails compilation or scenario replay.

### New Example Proof

Add at least three ad hoc examples in tests that are not maintained examples:

- a list app using `SOURCE`, `List/filter`, `List/map`, and dynamic rows,
- a formula/derived-value app using stdlib formula or dependency primitives,
- a small physics/collision app using `Geometry/intersects` and Boon-authored
  frame updates.

They must compile and run without editing compiler/runtime/backend code.

### Scenario Gates

Existing scenario gates remain required:

- Ratatui buffer,
- Ratatui PTY,
- native wgpu headless,
- native app_window/wgpu,
- Firefox WebGPU WebExtension.

All maintained examples must still have:

- source inventory proof,
- deterministic replay,
- semantic state assertions,
- screenshot/frame hashes,
- timing data,
- native app_window visible-surface proof,
- native playground interaction proof.

### Performance Gates

TodoMVC and Cells timing budgets remain hard gates.

If generated Rust from pure Boon becomes slower:

- optimize generated Rust hot paths,
- add dirty dependency scheduling,
- cache generic list folds/counts,
- batch render patch application,
- specialize stdlib primitives by type, not by app family.

Do not reintroduce Rust app families to regain speed.

## Implementation Phases

### Phase 1: Freeze Current Behavior And Add Stronger Failing Gates

1. Record current `cargo xtask verify all` as the behavior baseline.
2. Add static anti-cheat checks for semantic family structs/functions.
3. Add mutation probes for list/grid/dynamics behavior and scene construction.
4. Add tests that prove the current family structures are blockers.

Expected outcome: the new gate should initially fail until the family runtime is
removed or converted.

### Phase 2: Introduce Generic Boon-To-Rust Executable IR

1. Define typed executable IR operations for literals, records, lists, state,
   sources, events, conditions, loops, functions, and stdlib calls.
2. Lower current HIR/app IR into this generic executable IR.
3. Generate Rust app modules for counter and interval from that IR.
4. Keep old family runtime only behind temporary internal comparison tests.

Hard gate: counter and interval pass all platforms without scalar/clock runtime
families or a full generic VM.

### Phase 3: Move TodoMVC Entirely Into Boon

1. Encode todo state, filtering, editing, toggling, clear completed, counts, and
   footer visibility as Boon state/functions.
2. Build row and footer scene nodes in Boon.
3. Generated Rust executes Boon-authored list/state/scene logic using only
   generic bridge and stdlib APIs.
4. Remove list/repeater family runtime and list UI renderer.

Hard gate: TodoMVC scenarios and speed budgets pass; mutation probes prove
filtering/toggle/clear/scene behavior comes from source.

### Phase 4: Move Cells Entirely Into Boon

1. Encode selected cell, formula text, dependency graph, recalculation, visible
   viewport, and formula bar scene in Boon.
2. Write formula parsing/evaluation in Boon source, or call only an explicit
   reusable stdlib library API if one exists; do not keep a hidden Rust Cells
   formula engine.
3. Remove matrix/grid semantic runtime family.
4. Keep renderer grid lines as generic scene primitives only.

Hard gate: Cells 7GUIs scenarios and timing budgets pass; mutation probes prove
formula/dependency/view behavior comes from source.

### Phase 5: Move Pong And Arkanoid Entirely Into Boon

1. Encode frame state, input state, ball motion, paddle motion, collisions,
   brick removal, score/lives/reset, and scene nodes in Boon.
2. Use only generic geometry/math stdlib helpers.
3. Remove dynamics/kinematics semantic runtime family.
4. Renderer draws generic rectangles/text/layers only.

Hard gate: game scenarios prove held controls, autonomous frames, collisions,
brick removal, no extra Pong paddle, and frame hashes.

### Phase 6: Remove Temporary Compatibility And Rebaseline

1. Delete temporary comparison paths and aliases.
2. Update expected IR/source inventory/frame artifacts.
3. Strengthen anti-cheat gate so the removed families cannot return.
4. Run full verification.

Required final commands:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask generate
cargo xtask verify all
jq '.' target/boon-artifacts/success.json
jq '.' target/boon-artifacts/verify-report.json
jq '.' target/boon-artifacts/boon-powered-gate.json
```

## Definition Of Done

Done means all of these are true:

- No maintained-example Rust business logic remains.
- No semantic runtime families remain for list/repeater, matrix/grid, or
  dynamics/kinematics.
- Examples are transpiled from Boon to generated Rust app modules that use only
  generic bridge and stdlib functions.
- Scene trees are built by Boon source and rendered by generic Rust renderers.
- New ad hoc examples using supported constructs run without compiler/runtime
  edits.
- Static anti-cheat gate passes.
- Mutation probes pass.
- `cargo xtask verify all` passes and writes
  `target/boon-artifacts/success.json`.
- Timing, replay, source inventory, screenshot/frame hash, native app_window,
  and Firefox WebGPU gates all pass.

## /goal Prompt

Use this as the new `/goal` message:

```text
Continue from the current repo state in /home/martinkavik/repos/boon-rust and implement docs/plans/pure_boon_stdlib_migration_plan.md as the source of truth.

Treat it together with IMPLEMENTATION_PLAN.md, docs/plans/boon_powered_migration_plan.md, docs/plans/generic_boon_language_plan.md, docs/plans/native_gui_playground_plan.md, AGENTS.md, and prompter.json. The new stricter target is Boon-to-Rust transpilation with pure Boon application semantics: Rust may implement the compiler, Rust code generator, small deterministic runtime bridge, generic stdlib primitives, SOURCE plumbing, generic scene renderer, backends, verification, timing, and screenshots, but Rust must not implement maintained-example business behavior or semantic app families. Prefer generated Rust app modules over a full generic VM unless a small internal evaluator is explicitly justified as a compiler implementation detail.

Do not change cosmic-background-launch command-line parameters in this repo yet; the helper/compositor update is being handled separately and will be available after reboot. Continue using the current AGENTS.md launch contract until explicitly told otherwise.

Remove the remaining Rust semantic family hacks: TodoMVC-shaped list/repeater behavior, matrix/grid spreadsheet behavior, dynamics/kinematics game physics behavior, and Rust scene construction that knows todo rows, spreadsheet formula bars, paddles, balls, bricks, counters, or interval labels. Keep generic list primitives, keyed dynamic source ownership, and generic repeated scene rendering only when driven by Boon-authored data and scene code. Replace app behavior with Boon-authored state transitions and scene construction transpiled into Rust app modules that call only generic bridge and stdlib APIs. Generic Rust renderers may only draw scene primitives and backend details.

Follow the phases in docs/plans/pure_boon_stdlib_migration_plan.md:
1. freeze current behavior and add stronger failing anti-cheat gates,
2. introduce generic Boon-to-Rust executable IR and code generation,
3. move TodoMVC entirely into Boon,
4. move Cells entirely into Boon,
5. move Pong and Arkanoid entirely into Boon,
6. remove temporary compatibility and rebaseline artifacts.

Preserve all existing hard gates. All maintained examples must remain runnable, clickable/input-capable, and automatically verified by human-like scenarios on Ratatui buffer, Ratatui PTY, native wgpu headless, native app_window/wgpu, and Firefox WebGPU through the WebExtension/native-messaging harness. TodoMVC and Cells timing budgets remain hard gates. Do not regain speed by reintroducing Rust app families; optimize generated Rust, dirty dependency scheduling, stdlib primitives, or renderer batching instead.

Add deterministic proof that the implementation is pure: static anti-cheat scans must reject maintained-example names and forbidden family structures in implementation files, mutation probes must prove list/grid/game/view behavior depends on Boon source, and at least three new ad hoc non-maintained examples must run without compiler/runtime/backend edits: one list app, one formula/derived-value app, and one small physics/collision app.

Run relevant checks after each meaningful step and fix breakage instead of working around it. Required final verification:
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask generate
cargo xtask verify all
jq '.' target/boon-artifacts/success.json
jq '.' target/boon-artifacts/verify-report.json
jq '.' target/boon-artifacts/boon-powered-gate.json

Do not create temporary clean checkouts/worktrees for routine verification. Do not perform Git write operations unless explicitly requested. Stop only when cargo xtask verify all passes in the current development checkout, success artifacts prove all gates, and an evidence-backed code review shows Rust contains only compiler/runtime/stdlib/renderer plumbing and no maintained-example business logic or semantic app families. If blocked, leave the repo coherent and report the exact blocker, evidence, and next required command/change.
```
