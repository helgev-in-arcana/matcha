# AGENTS.md

Guidance for Codex working in this repository. **This file is an index, not a knowledge
base.** It is loaded into every session, so it stays small; everything else is retrieved on demand.

> **Maintenance rule.** A session's outcome goes in two places: one entry in `.agent/journal/` (what
> happened, what was tried, what was refuted) and an update to the affected `.agent/*.md` topic file
> (current facts only). **Touch this file only when the routing index, the commands, or the
> silent-failure list changes.** Do not append session narrative here — that is what made the
> previous version 165 KB.
>
> Prefer putting durable knowledge in the **module's own `//!` docs** over `.agent/`. This codebase
> already documents itself well there, and a fact next to the code it describes does not go stale
> silently. `.agent/` is for what has no single home in code.

## Project Overview

**Matcha** is a Rust GUI framework with an Elm-inspired architecture, built on wgpu (GPU rendering)
and winit/baseview (windowing). Intended as the frontend for a video editor project, and as the
engine behind a portfolio site on the web.

Two stacks live side by side:

- **`matcha-ecs` + `matcha-ecs-widgets`** — the current framework (bevy_ecs based). **All new work
  happens here.**
- **`matcha-tree` + `matcha-tree-widgets`** — the original tree-based implementation, superseded and
  kept only for reference. It does not build against current `wgpu` in every configuration (its
  `suzuri`-backed text modules are commented out). Do not extend it.

## Workspace Structure

| Crate | Role |
|---|---|
| `render-interface` | **Upstream rendering contract.** Borrowed `Scene`, typed content IDs, private resource definitions, GPU preparation contexts. Depends on no Matcha implementation crate. |
| `matcha-paint` | **CPU paint assembly.** Immutable bitmaps and UI-local paint trees resolve into the flat rendering contract. No dependency on `renderer` or `gpu-utils`. |
| `matcha-ecs` | **Framework core.** `UiEcs` driver, `Widget`/`Scope`/reconcile, layout protocol, picking, focus, keyboard/IME routing, clipping, render dispatch. Depends on no widget crate and no text engine. |
| `matcha-ecs-widgets` | **Widget implementations** and everything policy-shaped: containers, box decoration, sizing/flex, text widgets, scroll view, animation and interaction plugins. One-way dependency on `matcha-ecs`. |
| `matcha-web` | The wasm entry point — the whole page as one `<canvas>`. A **binary** crate (Trunk builds `--bin`), also runnable natively. Owns the embedded font. |
| `matcha-window` | Windowing abstraction over winit (default) and baseview, plus a `headless` backend for tests. Owns `WindowSurface`, input/IME event types, clipboard backends. |
| `renderer` | `SceneRenderer` executes the current contract and owns GPU resources. `CoreRenderer` and the atlas/tree API remain for the legacy stack and regression comparisons. |
| `gpu-utils` | `Gpu` context and `TextureAtlas`. |
| `glyph-cache` | Generic fixed-capacity LRU with per-batch eviction protection. wgpu-free. |
| `matcha`, `matcha-tree`, `matcha-tree-widgets`, `shared-buffer`, `utils` | Legacy tree stack and its support crates. Reference only. |

## Where to read next

Read the **one** file whose trigger matches. Each topic file is a map plus the facts that are not in
the code; follow its pointers into module `//!` docs for detail.

| Read this | When you are |
|---|---|
| [.agent/architecture.md](.agent/architecture.md) | Touching `matcha-ecs` core, adding a widget, or asking how a frame is produced / how a value reaches a `RenderItem` builder |
| [.agent/widgets.md](.agent/widgets.md) | Looking for an existing widget, or deciding whether something deserves a new widget type |
| [.agent/layout.md](.agent/layout.md) | Working on `Layout` impls, `Constraints`/`Measured`, sizing, flex distribution, clipping or scrolling |
| [.agent/input.md](.agent/input.md) | Working on picking, focus, keyboard/IME, pointer/hover/drag, tab order or the clipboard |
| [.agent/text.md](.agent/text.md) | Working on `Text`/`RichText`/`TextBox`, glyph rasterisation, or fonts |
| [.agent/rendering.md](.agent/rendering.md) | Touching `renderer/`, WGSL, atlases, mask chains, or upgrading wgpu |
| [.agent/web.md](.agent/web.md) | Building or debugging the wasm/WebGPU target, or touching anything with a `wasm32` cfg |
| [.agent/testing.md](.agent/testing.md) | Writing or running tests, or verifying something visual |
| [.agent/gaps.md](.agent/gaps.md) | Asking "is X supported / why isn't X done / what's next" |
| [.agent/journal/](.agent/journal/) | Doing archaeology: why a thing is the way it is, or what was already tried and refuted |
| [.agent/plans/](.agent/plans/) | Wanting the full design rationale behind a landed feature (Japanese, per-feature plan files) |
| [.agent/archive/](.agent/archive/) | **Rarely.** Superseded design docs, kept for history. Partly wrong — see the README there |

## Commands

```bash
cargo build --workspace --examples
```

```bash
cargo test --workspace --exclude shared-buffer -j 2
```

```bash
cargo build -p matcha-web --target wasm32-unknown-unknown --release
```

The wasm command applies only to checkouts containing `matcha-web`; it is absent from this
main-based branch. Current renderer proofs: `cargo test -p renderer --test scene_contract -j 2`,
`cargo run -p renderer --example scene_gallery -- target`, and
`cargo run -p matcha-ecs --example showcase -- --offscreen target/showcase-scene.png`.

Machine- and repo-specific quirks that will otherwise cost a debugging cycle:

- **`-j 2` on this machine.** A full-parallelism test run hits a Windows "page file too small" host
  error partway through. Not a code problem.
- **`shared-buffer` is excluded**: pre-existing doctest failures in the legacy stack, unrelated to
  any current work.
- **Never `--workspace` for wasm** — always `-p matcha-web`. `matcha`/`matcha-tree` pull
  `tokio/rt-multi-thread`, which does not exist on that target.
- **Never run `trunk build` while `trunk serve` is running** — both write `dist/`, and the race
  leaves the staging directory broken. Recovery: stop trunk, delete `dist/`, restart.
- `cargo test --workspace` occasionally flakes one `renderer` GPU test when several test binaries
  grab the adapter at once. It passes in isolation and on re-run.
- Examples: `matcha-ecs/examples/showcase.rs` is the single demo (it replaced ten per-feature ones).
  Run it to check anything visual.

## Fails silently — check these first

Symptoms with no error message, no log line, and no compile failure:

1. **`matcha_ecs_widgets::default_systems()` not registered** via
   `UiEcs::with_pre_layout_systems(..)` → exit fades never despawn, text boxes never re-lay-out,
   carets never blink, colour transitions never advance.
2. **No default font registered** (`WithDefaultFont::with_default_font`) → on the web, *all* text
   draws nothing, everywhere. A browser exposes no font database and both text stacks degrade
   silently. See [.agent/text.md](.agent/text.md).
3. **`trunk serve` only watches `matcha-web/`** → editing `matcha-ecs`/`renderer`/`matcha-window`
   does not rebuild, and the browser keeps serving the old wasm. Restart it.
4. **Integer vertex outputs in WGSL without `@interpolate(flat)`** → compiles natively, rejected by
   the browser. See [.agent/rendering.md](.agent/rendering.md).
5. **A widget carrying `ManualDespawn` with no system that despawns it** → leaks. Attach it only
   when an exit animation is actually configured.

## Coordinate System

UI logic is **Y-down**, origin top-left, unit = **UI pixels** (physical pixels ÷ `UiScale`).
`CoreRenderer` converts UI space → NDC. **Never flip the Y axis inside a widget or a layout.**

## Coding Guidelines

- Prefer `expect("Concrete reasons why this operation is guaranteed not to panic.")` or `?` over
  `.unwrap()`.
- Match the surrounding code's comment density and idiom. This codebase documents *why*, at module
  level, in `//!` blocks — extend those rather than writing a parallel document.
- Rejected alternatives are worth one line where the decision lives ("X was tried and refuted —
  journal 2026-07-10"). They are what stops the next session re-deriving them.
