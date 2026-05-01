# Native GUI Playground Plan

This plan extends `IMPLEMENTATION_PLAN.md` for the native/browser playground experience. The native playground must be a graphical GUI application, not a terminal transcript rendered into an app_window surface.

## Outcome

- `cargo xtask playground native [--example <name>]` opens a real `app_window`/`wgpu` GUI playground.
- The playground has a graphical shell with an example sidebar, toolbar/status area, preview surface, and visible control hints.
- The maintained examples are selectable and interactive: `counter`, `counter_hold`, `interval`, `interval_hold`, `todo_mvc`, `todo_mvc_physical`, `cells`, `pong`, and `arkanoid`.
- Example business logic stays in Boon source files. Rust may implement only generic runtime, rendering, timing, hit testing, input dispatch, and verification plumbing.

## Native GUI Requirements

- Native rendering uses `app_window`, `wgpu`, WESL, `wgsl_bindgen`, and `glyphon`; do not add winit, SDL, Sokol, Chromium, Playwright, WebGL2 fallback, or a DOM renderer for native.
- The renderer presents graphical framebuffer pixels, not a text-only terminal frame.
- Clicks must target actual visible controls. Counter must increment only from its button; TodoMVC must use input, checkbox, row, remove, filter, toggle-all, and clear-completed regions; Cells must use grid cells; games must use keyboard controls.
- Interval examples tick from live host time in manual playground mode and virtual time in deterministic verification mode.
- Pong and Arkanoid advance autonomously on a fixed frame tick in manual playground mode. They must not require mouse clicks to progress.

## TodoMVC Visual Target

- TodoMVC should match the classic web TodoMVC composition: centered 550px panel, large `todos` heading, input row, toggle-all, rows with checkboxes and remove buttons, footer with item count, filters, clear-completed action, and help text.
- Reference assets are checked in under `examples/todo_mvc/`:
  - `reference_700x700_(1400x1400).png`
  - `reference_metadata.json`
  - `expected.visual.json`
- The reference hash is protected. A changed reference image is a verification failure unless the expected hash is deliberately updated with a documented design change.

## Verification Gates

- `cargo xtask verify all` must generate GUI artifacts under `target/boon-artifacts/<example>/<platform>/`, including frame PNG/hash data, semantic/source inventory evidence, scenario traces, and timing data.
- Native app_window verification must cover the maintained example set, not a counter-only smoke.
- Native app_window verification must launch the real app_window/wgpu RGBA path
  for every maintained example in a fresh helper process and write
  `visible-surface-frame.json`. That proof must read back the actual app_window
  surface texture before present, prove nonblank/color-diverse pixels, and prove
  the final live surface size still matches the rendered frame size.
- Native app_window verification must write `playground-interactions.json` for
  every maintained example. These scenarios must drive the native playground
  shell through app_window-shaped mouse/key samples: sidebar/example selection,
  visible-control hit testing, typed text, keyboard controls, live interval/game
  advancement, semantic assertions, and frame hashes after steps.
- TodoMVC requires multiple playground scenarios: add via typed input, reject
  whitespace-only input, edit a row, remove a row, toggle checkbox, use filters,
  clear completed, and prove outside clicks do not mutate todos.
- Native GUI verification must fail if the captured frame is blank, solid, red/error-only, terminal-text-only, or missing the visual controls for the selected example.
- TodoMVC must pass deterministic state, source inventory, replay, timing, screenshot/hash, and visual comparison gates.
- Browser Firefox WebGPU and native headless wgpu should use the same graphical renderer path where possible so visual regressions are caught before manual testing.

## Manual Testing Commands

```sh
cargo xtask playground native --example todo_mvc
cargo xtask playground native --example cells
cargo xtask playground native --example pong
cargo xtask playground native --example arkanoid
```

Use `Esc` to quit, `Tab` or `PageDown` to switch examples, `PageUp` to go back, and `F1` through `F9` for direct example selection.
