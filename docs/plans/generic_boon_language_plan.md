# Generic Boon Language Implementation Plan

This plan describes the remaining gap between the current Boon-powered
maintained examples and a genuinely generic Boon implementation.

It is intentionally narrower than `IMPLEMENTATION_PLAN.md`: this document is
about removing the current family-recognizer architecture and replacing it with
general Boon parsing, lowering, execution, rendering, and verification. It must
be read together with:

- `IMPLEMENTATION_PLAN.md`
- `docs/plans/boon_powered_migration_plan.md`
- `docs/plans/native_gui_playground_plan.md`

## Current Honest State

The maintained examples are no longer just isolated handwritten playground
screens, and the runtime has real generic pieces: source inventories, host
contracts, `SourceBatch` dispatch, stale dynamic owner checks, render IR, native
hit targets, screenshot/frame artifacts, and multi-backend verification.

However, the implementation is not yet a general Boon compiler/runtime. The
compiler currently recognizes a small number of app families and creates a
`ProgramSpec` for them. New Boon code works only when it fits one of those
recognized families.

The current supported families are:

- scalar action accumulator,
- clock/tick accumulator,
- TodoMVC-like dynamic sequence/list app,
- Cells-like dense grid app,
- Pong/Arkanoid-like kinematics app.

This means the current state is useful but still too shape-driven. The next goal
is to make the behavior come from generic Boon semantics rather than Rust
recognition of app families.

## Problems To Fix

### 1. Parser Is A Scanner, Not A Full Language Parser

Current problem:

- The syntax layer collects useful facts such as `SOURCE` leaves, module calls,
  text literals, simple records, simple state steps, and `List/map` aliases.
- It does not fully parse Boon expressions as semantic syntax.
- Core user-facing constructs from `IMPLEMENTATION_PLAN.md` are not represented
  as proper typed nodes: `WHEN`, `THEN`, `WHILE`, `HOLD`, `LATEST`, `LIST`,
  `BLOCK`, records, tags, numbers, text, function calls, and module calls.

Resolution:

- Introduce a real AST for the allowed Boon surface syntax.
- Keep spans on every node so diagnostics and generated provenance stay useful.
- Represent all allowed constructs explicitly:
  - literals: text, number, bool, tag,
  - paths and field access,
  - records,
  - function/module calls,
  - pipeline calls,
  - `BLOCK`,
  - `WHEN`,
  - `THEN`,
  - `WHILE`,
  - `HOLD`,
  - `LATEST`,
  - `LIST` or the plan-approved list form,
  - `SOURCE`.
- Reject syntax outside the plan instead of silently ignoring it.

Hard gates:

- Parser fixture tests cover each allowed construct.
- Negative parser fixtures reject forbidden syntax and unsupported sugar.
- No example may pass because unparsed text was ignored.

### 2. HIR Lowering Is Too Shallow

Current problem:

- `boon_hir::lower` is effectively a wrapper around parsed syntax.
- The compiler still derives behavior by looking for patterns in parsed records
  and module-call names.
- There is no general representation of state cells, event boundaries,
  dependencies, list transforms, or view functions.

Resolution:

- Lower AST into a typed HIR with:
  - explicit value expressions,
  - source reads,
  - state cells,
  - holds,
  - event-triggered transitions,
  - list-producing operations,
  - derived views,
  - render tree expressions,
  - function definitions and calls,
  - dependency edges.
- HIR must be independent of example names and specific example shapes.
- HIR must preserve source spans and provenance.

Hard gates:

- HIR snapshot tests for every maintained example.
- HIR negative tests for unbound sources, conflicting source shapes, unknown
  host source fields, ambiguous producers, and unsupported constructs.
- A new simple example should not require editing compiler family-recognition
  code if it uses already-supported Boon constructs.

### 3. ProgramSpec Family Recognition Must Be Replaced

Current problem:

- `ProgramSpec` has fixed surface kinds such as `ActionValue`, `ClockValue`,
  `Sequence`, `DenseGrid`, and `Kinematics`.
- `program_spec` chooses one of those families from structural clues.
- Runtime dispatch and rendering then follow those fixed families.

This is the largest remaining genericity gap. Even if the names are no longer
example-specific, the implementation is still family-specific.

Resolution:

- Replace family-specific `ProgramSpec` with a general app IR.
- The app IR should contain:
  - typed state slots,
  - source inventory and host binding table,
  - event handlers as lowered expression graphs,
  - derived values,
  - list values with stable identity,
  - render tree expressions,
  - dependency graph for dirty recomputation,
  - deterministic clock/frame inputs as sources.
- The runtime should execute app IR generically.
- Specialized optimizations are allowed only below the semantic layer, for
  example dirty-bit scheduling, incremental list counts, cached formula
  dependencies, or renderer batching.

Hard gates:

- There is no `match` or dispatch path that chooses behavior by maintained
  example family.
- Adding a source-level feature to an example changes behavior through generic
  IR execution.
- Removing the source-level expression that implements a behavior breaks that
  behavior.

### 4. List Semantics Are Pattern-Specific

Current problem:

- TodoMVC works because the compiler/runtime recognizes a dynamic sequence
  family.
- The plan requires real list behavior for operations such as append, remove,
  retain, map, and count.
- Current support does not amount to a general list interpreter/lowerer.

Resolution:

- Implement list values as first-class HIR/app-IR values with stable item
  identity.
- Implement planned list operations generically:
  - `List/append(item: expr)`,
  - `List/remove(item, on: expr)`,
  - `List/retain(item, if: predicate)`,
  - `List/map(item, new: expr)`,
  - `List/count()`.
- Make dynamic source families derive from item identity, not from TodoMVC
  assumptions.
- Ensure stale events for removed/replaced dynamic owners are rejected.

Hard gates:

- Dedicated list operation tests independent from TodoMVC.
- TodoMVC mutation probes still prove that list behavior disappears when the
  relevant Boon list expression is removed or changed.
- New list examples can be added without Rust runtime branches.

### 5. State And Event Semantics Are Not General Enough

Current problem:

- The runtime has deterministic `SourceBatch` behavior and source validation.
- But event handlers are still fixed to current supported families.
- `HOLD`, `LATEST`, `THEN`, `WHEN`, and `WHILE` need to become generic
  execution semantics, not pattern hints.

Resolution:

- Implement a generic turn machine:
  1. validate all source emissions,
  2. update controlled source state,
  3. evaluate source-triggered handlers in deterministic order,
  4. use previous committed state for `HOLD`,
  5. expose source state after controlled-source sync,
  6. commit state atomically per turn,
  7. recompute dirty derived values,
  8. render from app IR.
- Implement deterministic ordering for same-batch source events.
- Keep the runtime synchronous. Do not add async/channels inside Boon runtime.

Hard gates:

- SourceBatch ordering tests for multiple events in one batch.
- Controlled input sync tests for text input, checkbox, focus/blur, and key
  repeat.
- `HOLD` and `LATEST` tests that prove previous/current boundary behavior.
- No runtime branch may mention TodoMVC, Cells, Pong, Arkanoid, counter, or
  interval as a semantic concept.

### 6. Rendering Is Not A General Document/Element Renderer

Current problem:

- Render IR exists and backends render it.
- But the scene still comes from fixed surface families rather than arbitrary
  lowered Boon view structure.
- Native playground visual quality is better now, but it must remain driven by
  Boon view data after genericization.

Resolution:

- Lower Boon view functions into renderer-neutral scene IR.
- Implement generic elements required by the plan:
  - buttons,
  - text inputs,
  - checkboxes,
  - labels/text,
  - lists,
  - grid/cells,
  - game/physical debug primitives if represented by planned Boon constructs.
- Hit targets must be emitted from the same render tree that creates pixels.
- Backends may only draw generic primitives.

Hard gates:

- Native app_window, native headless wgpu, Ratatui, and Firefox WebGPU all
  consume the same semantic scene output.
- Screenshot/frame hash checks fail on blank, solid, red/error-only, text-only,
  or stale frames.
- Manual playground behavior and automated scenarios use the same hit targets.

### 7. Cells Formula Semantics Are Too Limited

Current problem:

- Cells supports a useful subset of spreadsheet behavior.
- Formula parsing/evaluation is still limited and tied to current expected
  formulas.

Resolution:

- Represent cell values and formulas as generic Boon data.
- Implement a formula evaluator only for constructs explicitly allowed by the
  plan.
- Build a dependency graph for cell references and ranges.
- Support cycle/error reporting deterministically.
- Keep formulas and 7GUIs behavior in `examples/cells/source.bn`.

Hard gates:

- Formula unit tests independent from the Cells example.
- Scenario tests type values and formulas character by character.
- Dependency recalculation and range formulas are proven by semantic assertions
  and screenshots.

### 8. Kinematics Must Come From Boon Semantics, Not A Physics Family Shortcut

Current problem:

- Pong and Arkanoid are no longer supposed to be handwritten example branches,
  but the current kinematics support is still a fixed semantic family.
- That is better than app-specific code, but still not general Boon execution.

Resolution:

- Express frame advancement, collision decisions, paddle movement, scoring,
  lives, and brick removal through generic Boon state/list/control semantics.
- Keep physics deterministic.
- Keep backend drawing generic.

Hard gates:

- Mutation probes prove that removing/changing source physics expressions breaks
  physics.
- Scenario tests prove paddle motion, bounce, brick removal, scoring/lives, and
  frame hashes.
- No Rust code implements Pong or Arkanoid rules as named behavior.

### 9. Verification Is Strong For Maintained Examples But Not Generic

Current problem:

- Existing verification scenarios are intentionally example-aware.
- That is acceptable for user-level testing, but not enough to prove a generic
  language implementation.
- New examples require verifier edits.

Resolution:

- Keep example-aware scenarios for user behavior and visual correctness.
- Add language-level deterministic tests that are not tied to maintained
  examples:
  - parser fixtures,
  - HIR fixtures,
  - source binding fixtures,
  - list semantics fixtures,
  - state/event semantics fixtures,
  - render-tree fixtures,
  - mutation probes.
- Add a small generated-example corpus where each example exercises one feature.
- Add a verifier mode that compiles and executes every example from
  `examples/manifest.json` without hand-written semantic branches.

Hard gates:

- `cargo xtask verify all` runs both:
  - human-like scenarios for maintained examples,
  - language feature fixtures for generic Boon semantics.
- Reports distinguish app scenario failures from language semantic failures.
- Old success artifacts are rejected when inputs, compiler, runtime, scenarios,
  shaders, or expected artifacts changed.

## Implementation Phases

### Phase 0: Freeze Current Honesty Gates

Do this before changing architecture.

- Keep `crates/boon_verify/tests/boon_powered_gate.rs`.
- Add any missing files from this plan to the protected anti-cheat scan if the
  implementation creates new semantic/runtime/render files.
- Make the gate report family-recognizer code as a known remaining limitation,
  without blocking all work until replacement is ready.
- Keep mutation probes for TodoMVC and Cells.

Success:

- Current maintained examples still pass.
- The report honestly says which genericity gaps remain.

### Phase 1: Real AST And HIR

- Replace shallow syntax scanning with a real parser for planned constructs.
- Lower AST into typed HIR.
- Preserve source inventory extraction and host binding checks.
- Keep the old compiler path temporarily only as a compatibility layer while
  new HIR snapshots are brought up.

Success:

- Parser/HIR fixtures pass.
- Maintained examples compile through AST/HIR.
- Unsupported syntax fails with clear diagnostics.

### Phase 2: Generic State, Event, And List Runtime

- Introduce generic app IR for state cells, source handlers, derived values,
  lists, and render expressions.
- Implement `HOLD`, `LATEST`, `WHEN`, `THEN`, `WHILE`, `BLOCK`, and planned
  list operations.
- Migrate counter, interval, and TodoMVC to generic IR execution first.

Success:

- Counter, interval, and TodoMVC behavior come from generic app IR.
- TodoMVC speed gates still pass.
- Removing source expressions breaks the corresponding behavior.

### Phase 3: Generic Render Tree

- Lower view code into generic scene nodes.
- Remove fixed `SurfaceKind` rendering as the source of truth.
- Keep backend-specific drawing primitive code only.
- Migrate native playground to render the generic scene tree.

Success:

- Screenshots remain nonblank and visually useful.
- Hit targets come from generic scene nodes.
- Native playground interaction remains at least as responsive as current
  manual behavior.

### Phase 4: Cells And Formula Semantics

- Move Cells to generic list/grid/formula/state semantics.
- Add formula dependency graph and deterministic error/cycle behavior.
- Keep 7GUIs scenarios and visual checks.

Success:

- Cells passes formula, dependency, timing, source inventory, replay, and frame
  gates.
- Cells-specific Rust semantic code is gone.

### Phase 5: Generic Game/Kinematics Semantics

- Express Pong and Arkanoid through generic state/list/control/frame semantics.
- Remove fixed kinematics-family runtime behavior.
- Keep renderer primitive drawing generic.

Success:

- Pong and Arkanoid scenarios pass.
- Mutation probes prove source physics controls behavior.
- No named Pong/Arkanoid business behavior remains in Rust.

### Phase 6: Generic New-Example Workflow

- Document how to add a new example:
  1. create `examples/<name>/source.bn`,
  2. add expected artifacts generation,
  3. add it to `examples/manifest.json`,
  4. optionally add human-like scenario coverage if it has interactions,
  5. run `cargo xtask verify all`.
- Add one or more small feature examples that are not TodoMVC/Cells/game-shaped.

Success:

- A new example using already-supported constructs works without compiler or
  runtime branches.
- If the new example needs a new Boon construct, the compiler fails with an
  explicit unsupported-construct diagnostic.

## Required Review Standard

After implementation, perform an explicit code review before claiming success.
The review must answer these questions with file evidence:

- Does any Rust file implement maintained-example business behavior by name,
  title, source path convention, or fixed app family?
- Are compiler branches semantic language constructs or app-shape recognizers?
- Are list, source, state, and render semantics generic?
- Can a new example using existing constructs compile and run without changing
  compiler/runtime code?
- Do mutation probes prove examples depend on their Boon source?
- Do timing gates still pass after genericization?
- Do screenshots and frame hashes prove visible UI, not only semantic state?

Any "yes, but only because this example happens to match a hardcoded family" is
a failure.

## Required Commands

Run these before claiming completion:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask generate
cargo xtask verify all
```

Then inspect the artifacts instead of trusting their existence:

```sh
cat target/boon-artifacts/success.json
cat target/boon-artifacts/verify-report.json
cat target/boon-artifacts/boon-powered-gate.json
```

If native/browser windows are needed during testing, follow `AGENTS.md` and
launch the actual window-creating process through:

```sh
cosmic-background-launch --workspace boon-rust -- <command> [args...]
```

## Completion Criteria

The plan is complete only when all of these are true:

- `cargo xtask verify all` passes in the current development checkout.
- `target/boon-artifacts/success.json` exists and reports no failures.
- `target/boon-artifacts/boon-powered-gate.json` reports no handwritten
  business-logic violations and no failed mutation probes.
- All maintained examples still pass on required platforms.
- TodoMVC and Cells speed gates still pass.
- Native app_window screenshots prove visible graphical UI for every maintained
  example.
- A new example using already-supported constructs can be added without editing
  compiler/runtime semantics.
- The final review explicitly confirms that maintained-example behavior is
  driven by Boon source through generic compiler/runtime semantics.
