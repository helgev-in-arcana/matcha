# Testing conventions

```bash
cargo test --workspace --exclude shared-buffer -j 2
```

`-j 2` because a full-parallelism run hits a Windows "page file too small" host error on this
machine. `shared-buffer` is excluded for pre-existing doctest failures in the legacy stack.

## The GPU-free convention

**Tests in `matcha-ecs/tests/` and `matcha-ecs-widgets/tests/` never touch a `wgpu::Device`.**
CPU Frame/Draw scheduling tests may invoke writers; generators must not execute. Assertions include:

- `RenderItem::revision` value comparisons — changed props advance it, unchanged props preserve it.
  Extracted frames share the immutable builder; no retained Scene cache exists.
- `LayoutOutput` sizes and `GlobalTransform` positions.
- Extract-level snapshot contents (order, size, opacity, clip indices).
- Pure functions directly (`geometry`, `distribute`, `sizing`, `shape` coverage, whitespace
  collapsing) — these live as `#[cfg(test)]` modules next to the code, which is also the only way to
  reach private types.

Anything that needs real pixels is verified the way this repo has always done it: run
`matcha-ecs/examples/showcase.rs` and look, or dump an offscreen render to PNG with a throwaway
diagnostic and delete it afterwards. Reusable feature proofs are retained as examples: showcase
--offscreen and renderer scene_gallery cover the new Scene contract. Interactive resize and IME are structurally untestable
headlessly and stay human-visual checks.

Where a pixel check matters, **diff numerically against a baseline built from the previous commit**
rather than eyeballing. The mask-chain rewrite was verified that way: a quad whose mask matches its
texture quad came out bit-identical, two nested clips lit exactly their intersection with zero
disagreeing pixels, and draw-time alpha composited in linear space (sRGB 0x80 at alpha 0.5 over
black reads 0x5c).

## Headless everything

The full app driver — real `UiEcs`, real `Adapter`, real input state machine — runs with no GPU and
no OS window.

- **GPU**: wgpu's NOOP backend via `gpu_utils::GpuDescriptor::noop()`. Needs `Backends::NOOP` **and**
  `backend_options.noop.enable = true`. The noop adapter reports `Features::all()`, so feature
  requirements and `CoreRenderer::new` pass unchanged.
- **Window**: `matcha-window`'s additive `headless` feature. `HeadlessWindow`'s
  `create_wgpu_surface` returns `Ok(None)`, so `get_surface_texture` returns `Ok(None)` and frames
  skip naturally. There is no headless `Adapter::run` — **the test is the event loop**, calling
  `adapter.init/resumed/create_surface/render/device_event/ui_command` directly.
  `cargo test -p matcha-window --features headless` (gated by `[[test]] required-features`).
- Backend features cannot be exclusive: dependencies and dev-dependencies feature-unify, so an
  exclusive backend collides with winit's `compile_error!`. Hence `headless = ["winit"]`.
- `matcha-ecs/tests/headless_app.rs` drives click → pick → reducer → re-view through the real
  `Adapter`. **A pre-click `adapter.render(id)` is required** — the picker updates in
  `MatchaSet::PreExtract`. View fn and reducer are fn pointers so the `UiEcs` type is nameable.

## Scene GPU contract

cargo test -p renderer --test scene_contract -j 2 -- --nocapture --test-threads=1

The tests require a real GPU and never silently skip to NOOP. MATCHA_TEST_BACKEND=dx12 or vulkan
selects a backend. The legacy CoreRenderer implementation is unchanged from the main base and is
used for numerical pixel parity. CPU pool/assembly checks live in render-interface/tests and
matcha-ecs/tests/scene_composition.rs. No GPU tests were added to the ECS/widget test folders.

The main-based checkout contains no matcha-web crate or uniform-params feature; wasm/browser
validation is not covered by native shader compilation or by the present proof suite.

## Test locations

| Where | What |
|---|---|
| `matcha-ecs/tests/` | Core: reconcile, layout, measure cache, hidden, traversal, focus, input, pointer, keyboard, tab order, clip, extract, draw size, viewport, ui scale, cursor, task, headless app |
| `matcha-ecs-widgets/tests/` | Per-widget cache invalidation and layout behaviour (moved here from core on 2026-08-02) |
| `renderer/tests/` | Real-GPU: culling/visibility, mask scale, uniform params, noop smoke |
| `matcha-window/tests/` | Headless backend |
| in-file `#[cfg(test)]` | Pure helpers and anything needing a private type |

## Native interface stress

cargo run -p matcha-ecs --example interface_stress -- target

This example invokes the production Scene assembly/backend, compares five GPU shape masks to the
pure CPU oracle, shares one generated background across three widgets, refreshes backdrop IDs on
redraw, verifies translated sampling and GPU-only relocation, and checks failed-assembly retention.
MATCHA_TEST_BACKEND=dx12 selects DX12; default is Vulkan. GPU assertions live here or in renderer
tests, preserving the GPU-free ECS/widget integration-test convention.

scene_contract includes deliberate contract-violation diagnostics: stale backdrop IDs and invalid
GPU copy commands are expected to exhibit their documented failure modes. Passing those tests
does not mean invalid producers are automatically repaired. The pixel-art case shows the cost of
expressing nearest-like sampling through geometry under the fixed linear-sampling ABI.

cargo run -p renderer --example image_diff -- BASELINE.png ACTUAL.png

Use release builds for performance measurements. Offscreen showcase prints assembly, CPU encode/
submit and GPU-wait wall times separately; these are not GPU timestamp measurements.
