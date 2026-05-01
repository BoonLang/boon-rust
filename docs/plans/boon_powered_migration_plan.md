# Boon-Powered Playground Migration Plan

This plan is a hard implementation contract for replacing the current
handwritten Rust example behavior with real Boon-powered execution.

## Goal

After this migration, the native playground should behave at least as well as
the current one, but the behavior and view structure must come from
`examples/<name>/source.bn` through Boon parsing, lowering, and runtime
execution.

The maintained examples are:

- `counter`
- `counter_hold`
- `interval`
- `interval_hold`
- `todo_mvc`
- `todo_mvc_physical`
- `cells`
- `pong`
- `arkanoid`

## Non-Negotiable Rules

- Do not remove or weaken `crates/boon_verify/tests/boon_powered_gate.rs`.
- Do not whitelist current handwritten Rust example logic.
- Do not move the cheats to another Rust file.
- Rust may implement generic parsing, HIR/IR lowering, shape checking, turn
  execution, render IR application, input dispatch, hit testing, backend
  drawing primitives, app_window/wgpu/browser plumbing, verification, and timing.
- Rust must not implement TodoMVC, Cells, Pong, Arkanoid, counter, or interval
  business behavior as example-specific branches.
- Rust renderers must not draw handcrafted example screens. They must render
  generic Boon render IR / scene nodes.
- Verification scenarios may be example-aware, but they must drive public
  SOURCE/input boundaries and assert Boon-owned state/render output.

## Required Architecture

The implementation should converge on this flow:

```text
examples/<name>/source.bn
  -> AST with spans
  -> HIR with SOURCE inventory and host bindings
  -> renderer-neutral Boon app IR
  -> deterministic synchronous turn machine
  -> render patches / scene tree
  -> Ratatui, native wgpu/app_window, Firefox WebGPU backends
```

The generated/interpreted app may be optimized, but it must remain a generic
execution of lowered Boon IR, not a Rust `match example_name` or
`program.title.contains("Arkanoid")` implementation.

## Phase 1: Make Current Cheats Impossible To Miss

Deliverables:

- Keep `boon_powered_gate` in `cargo test --workspace`.
- Make `cargo xtask verify all` fail until the anti-cheat gate passes.
- Ensure reports explain that handwritten runtime/rendering example logic is the
  blocker.

Success criteria:

- Current handwritten example logic is reported as a failure.
- The failure cannot be bypassed by semantic state alone or old success
  artifacts.

## Phase 2: Real Boon Example Semantics

Deliverables:

- Expand parser/HIR only for planned Boon constructs already allowed by
  `IMPLEMENTATION_PLAN.md`.
- Lower all maintained `source.bn` files into a common app IR.
- SOURCE declarations and host bindings are derived from the lowered program.
- TodoMVC list behavior, filters, editing, checkbox/toggle-all/clear-completed,
  and footer state are implemented in Boon source.
- Cells formula dependencies, range formulas, selected cell, formula bar, and
  recomputation are implemented in Boon source.
- Pong and Arkanoid deterministic physics, paddles, scoring/lives, bricks, and
  frame advancement are implemented in Boon source.

Success criteria:

- Removing a relevant expression from `source.bn` changes/fails the example.
- There is no Rust fallback that preserves example behavior after Boon source is
  broken.

## Phase 3: Generic Runtime And Render IR

Deliverables:

- Replace `example_runtime_template.rs` example branches with generic generated
  or interpreted turn execution.
- Replace handcrafted WGPU preview functions with generic rendering of Boon
  scene/render IR.
- Preserve current native playground usability: sidebar, real GUI, visual
  TodoMVC, Cells grid/formula bar, Pong/Arkanoid controls, held-key repeat, and
  clean app_window close.

Success criteria:

- `boon_powered_gate` passes without weakening.
- Native app_window artifacts still show useful graphical frames for every
  maintained example.

## Phase 4: Performance And Hard Gates

Deliverables:

- Maintain or improve current TodoMVC and Cells timing paths.
- Optimize generic IR/runtime dirty recomputation where needed.
- Keep deterministic replay, source inventory, screenshot/frame hash, and timing
  artifacts.

Success criteria:

- `cargo xtask verify all` passes unattended.
- `target/boon-artifacts/success.json` exists.
- TodoMVC typing, checkbox, and toggle-all timing budgets pass on required
  platforms.
- Cells timing budgets pass on required platforms.
- Native app_window verification covers all maintained examples, not a
  counter-only smoke.

## Explicit Stop Conditions

Stop and report a blocker instead of hiding it if:

- A required Boon construct is ambiguous and cannot be inferred from the plan.
- A timing budget cannot be met without changing the language contract.
- A backend can only pass by reintroducing app-specific Rust behavior.
- `cargo xtask verify all` cannot run because a system prerequisite is missing;
  report the exact install command needed.
