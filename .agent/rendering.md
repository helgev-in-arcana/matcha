# Native rendering interface and GPU placement

Read render-interface/src/lib.rs for the contract. matcha-paint has been removed. Current widgets
emit native Scene values/updates with Object, PixelMask and Source definitions. No RenderNode or
Bitmap adapter exists on the ECS path. The legacy renderer/tree stack remains unchanged.

## Code map

- matcha-ecs/src/components/render.rs: retained Scene builders and in-place dynamic Scene writers.
- matcha-ecs/src/scene.rs: flat Scene embedding, source sharing, mask-index validation/rebasing.
- matcha-ecs/src/render.rs: extract, assemble, borrow into backend, submit/present coordination.
- matcha-ecs-widgets/src/shape_gpu.rs + .wgsl: native SDF/ring/three-box shadow generators.
- renderer/src/scene_resources.rs: texture rectangle leases and shared mesh-buffer intervals.
- renderer/src/scene_renderer.rs + .wgsl: preparation, packing, composition and GPU relocation.
- renderer/tests/scene_contract.rs: positive contracts plus explicit negative design diagnostics.
- matcha-ecs/examples/interface_stress.rs: actual widget/effect integration and CPU/GPU shape oracle.

## Ownership

Scene and resource definitions are CPU-side application data. Source fields/maps stay private.
Source cloning shares one Arc<Prepare> allocation, replacing Box<Prepare>; no Arc<Source> wrapper.
Separate retained widget Scenes can share definitions. ResourcePool::import/share_* explicitly
compose content IDs; public insert_* still rejects duplicate definitions in a single pool.
Import checks descriptor compatibility, not closure pointer equality. Semantic equality remains
part of the immutable-content-ID contract. Reconstructing an equivalent generator is legitimate.

The renderer owns final resident placement. Providers may own private shader pipelines and work
resources. They record Copy/Compute/Render into borrowed logical outputs; no Queue or final target
is supplied. Generators must initialize outputs completely and not retain their identities.
Exact-size/format/usage outputs are pooled within a frame after their placement copy is recorded.
Two simultaneous mesh outputs remain distinct even with equal sizes/usages. Scratch pools drop
at render completion/error, so a history of dimensions cannot accumulate indefinitely.

## Phase semantics without snapshot copies

All generation reads for a phase are recorded before its Object draws. The accumulation image
itself therefore supplies the phase-start snapshot; queue/encoder order protects those reads.
No full-viewport snapshot copy or separate snapshot attachment is needed. This relies on sources
not modifying/retaining the input or submitting work out of band. Later/lazy preparation would
need a different strategy. Culling still occurs after preparation to preserve first-use semantics.

Colour accumulation is RGBA16Float; six R8 mask work images cache a four-node prefix and a two-slot
ping-pong tail. Working attachments cost 14 bytes/pixel, excluding target/cache/driver resources.
Coincident non-overlapping mask meshes sample coverage by local UV. General mask meshes rasterize
coverage and multiply parents. Optional bounds limit clear/draw rectangles. Same-attachment draws
are batched in paint order. A reusable 112-byte-per-draw (aligned) uniform arena carries placements
and atlas UV rectangles. Per-frame bind groups are shared by page-view tuple. Filtering clamps to
texel centres of each resident rectangle, so adjacent allocations do not bleed.

## Placement and lifetime

Texture pages are separated by format (default edge 1024). Resources exceeding the page size get
larger pages, subject to device limits, with no implicit rescaling. Vertices/indices occupy leased
intervals in shared buffers (default page 256 KiB). Output copies preserve logical offset zero for
generators. Allocation Drop releases intervals; weak page indices permit empty pages to disappear.
Reuse is safe because later writes/copies follow earlier submitted reads on the same Queue.

compact_resources changes placement by GPU-to-GPU copying cached content. It does not invoke
Source callbacks, change IDs or reinterpret background-dependent resources. set_atlas_config
instead clears resident caches and changes the policy for subsequent regeneration.

Cache budget is soft logical bytes, not a VRAM cap. PlacementStats reports page/buffer capacity,
not driver allocation sizes or in-flight memory. Fewer pages can consume more capacity. Current
working sets are pinned and generation callbacks are rolled back on PrepareError; GPU validation
errors are a separate wgpu error channel. CPU Ok is not GPU validation/completion confirmation.

## UI integration constraints

RenderCtx supplies resolved placement and viewport size for backdrop producers. Object/mask
records in widget Scenes are local and are embedded once. Standard local sources ignore placement;
view/background/time-dependent definitions use RenderItem::dynamic to refresh content IDs while
reusing Scene storage. It schedules no redraw itself. ClipReset resets ancestor clipping for both
paint extraction and picking. It changes neither geometry nor ZIndex; custom Phase paint order
is not automatically an input order.

Fontdue glyph rasterization happens on GPU-content misses; swash's bounds and pixels are produced
together, so its native MaskSource retains the pixels. Image sources likewise retain decoded
pixels. These are provider-specific CPU algorithms, not a common Bitmap transport layer. Byte
image cache entries carry weak input ownership to prevent naked-address identity reuse.

See ../docs/native-render-interface-report.md and journal/2026-09-23-native-scene-stress.md for
experiments, counterexamples and design-feedback recommendations. This main checkout still has
no matcha-web crate. Native Vulkan/DX12 validation is not a browser compatibility claim.
