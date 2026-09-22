# 2026-09-23: borrowed Scene rendering interface

Request: implement the agreement in ChatGPT conversation
6aa53a2e-3634-83ee-a542-374f04789e80, starting from main and staying on a new local branch.

Base: main a7fd3f6c58fad19934231b52ee356828ef42ebba. Work branch:
codex/render-interface. Working tree initially clean. No fetch, push, PR or other GitHub action.
Existing branch tips were recorded and must remain unchanged.

## Agreement recovered

The latest messages supersede earlier Arc<Source> and duplicate-registration proposals:
borrow &Scene during render; UI owns reusable definitions; Source fields/maps private;
one definition per typed content ID; no generations; phases then array paint order;
independent parent-linked PixelMasks; all generators in a phase read its start snapshot;
no automatic generation from pool membership; arbitrary GPU recording to dedicated outputs.
G-buffer/history/old-ID hints were explicitly deferred in that conversation.

## Landed design

render-interface is upstream of both renderer and CPU paint assembly. matcha-paint contains
Bitmap, RenderNode, SceneBuilder and a custom Scene contribution callback. All current widgets
now produce CPU assets instead of atlas allocations. GuiRenderer uses a persistent SceneBuilder
and borrows its Scene into SceneRenderer. Threaded and inline presentation share the same method.
The legacy CoreRenderer/tree stack is unchanged and remains available as a numerical baseline.

Fixed ABI: Float32x3 position + Float32x2 UV, triangle list, optional u32 indices, premultiplied
linear compositing. Optional truthful conservative bounds and non-overlap declarations permit
backend optimizations while default descriptors preserve arbitrary mesh behavior. GPU-generated
meshes, sampled/uploaded/rendered/computed textures and arbitrary mesh masks all run through
the same contract. Image decoding now premultiplies in linear space before sRGB encoding.

## Experiments, failures and improvements

1. Straight implementation: dedicated cached resources, two full-viewport mask images, one
   buffer and render pass per draw. Small real-GPU contract tests passed. Actual showcase
   failed at scale: 2,015 object draws, 4,274 mask passes, Queue::submit OutOfMemory and Vulkan
   command-pool cleanup validation errors. Cache logical bytes were only 612,682; that number
   was not actual total GPU memory. Small-scene success was insufficient evidence.
2. One uniform arena, conservative bounds, partial clears and shared mask prefixes reduced
   overhead but alone still failed the full showcase. Do not claim buffer allocation alone
   was the isolated cause; the exact driver allocation was not determined.
3. Direct UV coverage for coincident non-overlapping meshes plus ordered attachment batching
   made the showcase succeed. Four prefix mask slots and two ping-pong tail slots keep deep
   general masks bounded independently of total node count. General/optimized projective
   results agree to at most one quantization step. Ten ancestors exercise the tail path.
4. A callback failing after an earlier successful preparation exposed the transactional
   requirement: unsubmitted resources must not remain cache hits. Entries carry a private
   creation-frame tag for rollback; all newly recorded entries are removed on failure.
   This is internal accounting, not an interface generation scheme.
5. Uniform GPU storage and CPU byte capacity are reused across frames. Source values remain
   directly stored in their persistent HashMaps; no per-Source Arc was introduced.

## Evidence

- Full workspace: 448 passed, 0 failed, 8 ignored (one existing unimplemented GPU-utils test and seven doctests); shared-buffer
  excluded by repository convention. All workspace examples built.
- Real contract tests passed on AMD Radeon RX 5700 XT, both Vulkan and DirectX 12, with the new
  backend requesting no optional wgpu features.
- Old CoreRenderer parity: 4,096/4,096 pixels identical, maximum channel difference zero,
  including a coverage gradient, clip and opacity; old renderer sources match main exactly.
- Six GPU-generated PoC images and the production GUI offscreen image are retained under
  docs/render-interface/. Commands and validation boundaries are in the report there.
- Normal native showcase remained alive for a bounded 10-second smoke run without stderr
  errors; the test-owned process was then stopped. Interactive IME/resize was not exercised.

## Scope and branch caveats

The inherited local .agent files described a newer web branch. This main checkout has no
matcha-web, uniform-params feature, or default-font facade. Do not run those missing commands
or claim a browser was verified. Updated topic maps state current facts. The user explicitly
requested repeatable PoCs/reporting, so the feature proof examples and selected outputs are
retained rather than treated as throwaway diagnostics.

Remaining work is optimization/extension, not missing agreed core semantics: GPU atlas/mesh
packing, bindless draw submission, G-buffer, history, MSAA/depth, GPU device-loss recovery,
and live input/resize verification. Cache budget is soft logical content bytes, not a hard
VRAM cap; viewport intermediates and driver overhead are additional.
