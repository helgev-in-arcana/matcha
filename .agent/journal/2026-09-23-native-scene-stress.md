# 2026-09-23: native Scene producers, resident placement and interface stress

Continuation of 0088c26 on codex/render-interface. The user rejected the RenderNode/Bitmap
bridge as too close to the old implementation, authorized framework changes and native wgpu
widget generation, requested texture/mesh placement in the renderer, then explicitly prioritized
experiments that feed back into the rendering-interface design. No GitHub operation is authorized
or performed. Existing branch tips must stay unchanged.

## Structural outcome

- Removed matcha-paint completely. Widgets build/update render-interface::Scene, with native
  Object/PixelMask/Source records. Local composition flattens immediately; no persisted paint tree.
- RenderItem retains Scene directly in its mutex. Static builders rebuild on invalidation;
  dynamic writers mutate retained Scene storage on redraw. GuiRenderer separates CPU assemble
  from GPU render. Failed assemblies prune stale dynamic definitions rather than accumulating them.
- Source::clone shares Arc<Prepare>, replacing Box<Prepare> with one allocation, not adding
  Arc<Source>. This supports independently retained widget Scenes. Import/share_* normalize
  contributors by content ID; ordinary insert_* still rejects duplicate pool definitions.
- An intermediate import implementation required closure Arc pointer equality. Refuted: two
  reconstructed definitions can legitimately have the same content ID. Final import checks ID
  and descriptor compatibility and relies on the existing semantic-content promise.
- RenderCtx gained final transform/viewport information: a backdrop generator could not locate
  its own region in the full-viewport snapshot using only local widget size. Objects stay local;
  the driver applies placement exactly once. Dynamic writers refresh view-dependent content IDs.
- ClipReset starts an independent clip scope for extraction and picking. It does not change
  geometry or ZIndex. An initial test incorrectly probed outside the layout-constrained button;
  explicit popup placement made the test exercise clipping rather than a missing hit rectangle.

## Native GPU producers

Solid colour is generated with an attachment clear. Rounded fills, asymmetric border rings and
shadows use widget-owned wgpu shaders. Shadow generation uses a private six-pass separable filter;
the CPU implementation remains an independent oracle. Five GPU/CPU cases were byte-identical on
the initial Vulkan run. Fontdue rasterization is deferred into MaskSource; swash supplies bounds
and pixels together, so its MaskSource retains pixels. Image decoding remains provider-specific
CPU work, stored in TextureSource, without a common Bitmap wrapper.

While inspecting image identity, found that a naked pointer/length key outlived its byte owner.
Allocator address reuse could return a different image's cached source. CachedImage now holds
weak input ownership and expires dead entries before lookup. A CPU lifetime test covers expiry;
the fix does not rely on reproducing a particular allocator address reuse.

## Backend placement and experiments

- Per-format texture pages (default 1024 edge), shared vertex/index buffer pages (256 KiB).
  Rectangle/interval leases free on drop, weak page indices release empty pages. Oversized
  sources get larger pages. Shader UV remapping clamps to the resident rectangle's texel centres.
- Generators still see isolated exact logical outputs at origin/offset zero. They are copied
  into resident placement on the GPU. Exact-descriptor outputs are leased/reused within a frame;
  equal-sized vertex/index outputs remain distinct while simultaneously borrowed. Pools clear
  after success/error so historical dimensions do not retain scratch storage forever.
- compact_resources relocates cached GPU content without callbacks or new IDs. Already submitted
  frames, relocations and subsequent writes are ordered on the same queue; old GPU handles are
  retained by wgpu commands. Callback-error rollback releases newly allocated placement leases.
- 64 small varied textures/meshes plus an oversized image forced 7 texture and 16 mesh pages.
  Repacking yielded 2 texture and 1 mesh pages with identical pixels and unchanged generation
  counts. However texture page capacity rose 46,468 -> 196,608 bytes: fewer pages != less memory.
- Expiry/refill retained live pixels and reused placement. One refill generated 32 textures and
  32 meshes with only one logical texture output allocation and one logical buffer allocation.
- Eight bind groups serve the 1,038 visible showcase draws. They are still 1,038 draw calls;
  seven draw-containing render passes do not imply seven draw calls.

## Snapshot experiment that simplified the implementation

All producer reads are recorded before the phase's Object writes, and outputs are isolated.
Therefore the accumulation image itself is a stable logical phase-start input under encoder
ordering. Removed full-viewport snapshot copies and the separate RGBA16Float snapshot image.
Positive first-use/multi-phase tests still pass. Working images are now 14 bytes/pixel (RGBA16
colour + six R8 masks), excluding target/cache/driver memory. This optimization would need review
if preparation became lazy or callbacks could submit/retain input access out of band.

## Deliberate counterexamples and limits

1. Reusing a backdrop consumer ID after changing its background produced stale red on a warm
   cache and blue after cache clear. This intentionally violates content identity. Fresh IDs fix
   it; the current API cannot infer the missing dependency. Consider explicit snapshot-version
   construction helpers while keeping content IDs distinct from recipe IDs.
2. A callback returning Ok recorded an invalid RGBA16->RGBA8 texture copy. CPU render returned Ok,
   and wgpu's error scope reported validation failure. Current Result is not GPU validation or
   completion status. A submission/error receipt is a separate design decision, not silently fixed
   by waiting for the GPU in every render call. Negative tests isolate and clear that cache.
3. Fixed linear filtering blurred a two-texel pixel-art image. Constant UV per texel quad produced
   the intended sharp result, but costs six vertices per texel. Efficient nearest/linear selection
   belongs to drawing semantics, not atlas placement. No sampler-policy field was added speculatively.
4. Private depth rendering and multi-pass work inside a producer work: a 3D cube uses its own
   vertex ABI/camera/depth buffer, then supplies colour to the UI. This does not add cross-Object
   depth, a shared G-buffer, or direct resource-ID dependencies between generators.

## Verification paths

renderer/tests/scene_contract.rs: positive phases/masks, projective fast/fallback parity, deep
chains, immutable cache reuse, failures/retry, packing/eviction/relocation, 1,536-vertex GPU
animation, private 3D, legacy pixel baseline, and explicitly labelled negative diagnostics.
Runs on Vulkan and DX12. interface_stress example runs real widget assembly on both backends:
shared GPU background once, three new effects/redraw, translated-backdrop oracle, no callback on
relocation, and bounded retention across 20 failed CPU assemblies followed by a correct retry.

The native showcase image matched the previous committed image at all 900,000 pixels. Debug
timings varied substantially and were dominated by CPU work. A release measurement gave warm
frames around 2.7-4.1 ms including completion wait; treat this as a bounded local measurement,
not a portable FPS guarantee or an old/new release benchmark. Final exact logs and updated
measurements belong in docs/native-render-interface/validation.txt.

See docs/native-render-interface-report.md for the final design-feedback report. Browser,
real IME/continuous resize, variable GPU-produced draw counts, scene-level G-buffer and automatic
device-loss recovery remain outside the verified scope.

Final validation: 457 passed, 0 failed, 8 pre-existing ignored. All workspace examples built.
Both Vulkan and DX12 passed 8 contract/stress tests and the native widget example. Five GPU/CPU
shape cases had maximum byte delta 0 on both. Final release showcase: cold 247 ms, warm
4.11 / 3.03 / 3.03 ms including completion wait; assembly 126-132 us. It used 3 texture pages,
1 mesh page, 8 bind groups, 7 draw batches for 1038 visible Object draws. Cold preparation
recorded 407 resources with 119 temporary textures and 1 temporary buffer; warm allocation/
generation and backend snapshot materialization counts were zero. Final release screenshot
matched the previous committed 900000 pixels exactly. Exact logs are in validation.txt.
