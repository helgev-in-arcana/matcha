# Rendering: the upstream Scene contract

Current ECS rendering is defined by render-interface/src/lib.rs. Read its module docs first.
The dependency direction is UI -> render-interface <- renderer. matcha-paint owns CPU-side
assembly; matcha-ecs::render::GuiRenderer owns the reusable SceneBuilder and SceneRenderer.
Widget builders receive CPU layout/interaction state, never atlas/device/queue handles.

## Module map

- render-interface/src/lib.rs: source/ID/phase/mask contracts, descriptors, GPU contexts, uploads.
- matcha-paint/src/lib.rs: immutable CPU Bitmap, local RenderNode, persistent SceneBuilder.
- renderer/src/scene_renderer.rs and .wgsl: GPU residency, preparation, composition, error rollback.
- renderer/tests/scene_contract.rs: real GPU contract, effects, projective fast/fallback parity,
  ten-level masks and unchanged legacy CoreRenderer pixel comparison.
- renderer/examples/scene_gallery.rs: six reproducible GPU images.
- matcha-ecs/examples/support/offscreen.rs: actual UI extraction/build/render proof.

## Current facts

Scene is borrowed synchronously. CPU Source values live directly in private HashMaps. IDs are
immutable-content identities. A complete pool is required even with warm GPU caches. Registration
alone never invokes a generator. Callbacks record commands through an encoder; submit stays in the
renderer. On callback error the command buffer is discarded and newly inserted cache entries are
removed. Invalid raw GPU commands remain subject to wgpu's validation/error model.

The vertex ABI is position Float32x3 + UV Float32x2, triangle lists, optional u32 indices. Y is down.
Transforms may be projective; z is not used as a depth buffer. Colour is premultiplied linear RGBA;
sRGB storage bytes encode premultiplied linear RGB. Object.opacity multiplies all four channels.
Images now premultiply after sRGB decoding instead of uploading straight-alpha bytes.

Each phase reads a frozen start image. Scratch colour/snapshot are RGBA16Float. Masks multiply
parent coverage, with no transform inheritance. Arbitrary overlapping mask triangles use maximum
coverage within one node. Six R8 viewport images bound chain working memory: four reusable prefix
slots, two ping-pong slots for deeper chains. Conservative mesh bounds limit clear/draw rectangles.
Coincident non-overlapping object/mask meshes sample local coverage directly. Consecutive draws
sharing an attachment are batched; parameters use one aligned uniform arena per frame.

bounds and non_overlapping are optional truthful geometry promises, not requirements. Defaults
preserve the general path. Wrong hints can change pixels, just as lying about generated content can.
All preparation happens before per-phase culling, so optimizations cannot change first-use snapshots.

GPU cache budget defaults to a soft 128 MiB of logical resource bytes. The current frame is pinned;
scratch images, driver allocation granularity and in-flight commands are additional memory. Pool
presence is a retention tie-breaker, not an actual-use timestamp. CPU bitmap caching is separate.

ThreadDriver and InlineDriver share GuiRenderer::render_extracted. A GUI renderer lock serializes
assembly and GPU command recording across windows. Window/surface lifetime still belongs to the
existing driver; Scene contains no window or ECS objects.

## Legacy and branch differences

renderer::CoreRenderer, render_node.rs and atlas helpers are unchanged from main a7fd3f6 and still
serve the old tree stack. They are not the current ECS renderer. Do not transfer their immediate,
quad-only or atlas ABI restrictions to SceneRenderer. This checkout has no matcha-web crate or
uniform-params feature. Old notes describing those branches are not current build instructions.

See journal/2026-09-23-render-interface.md for experiments/refuted attempts and
../docs/render-interface-report.md for the implementation report.
