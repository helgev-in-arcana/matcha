# 2026-09-23: implement framework-owned phases and lightweight draw writers

The user requested implementation of the agreed decisions. Continued codex/render-interface;
no other branch moved and no GitHub action. Removed widget-local Scene caches and dynamic Scene
writers. RenderItem now writes into Frame through Draw every redraw. Ordinary objects preserve
paint order; backdrop objects request the preceding image and the framework creates the phase
boundary. Scoped transforms/masks flatten immediately. Shared GPU resource IDs remain stable.

Provider caches retain shaped layouts, image definitions and shape/glyph resources. RenderItem
invalidation advances a value revision, avoiding the old cache-slot allocation. Source generator
sharing remains Arc<Prepare>; no per-Object Arc or retained Object wrappers were added. This is
not a performance verdict on every possible ownership strategy.

Initial direct-writer showcase assembly cost about 2 ms, with 19 allocations plus 1 reallocation
in steady state. Found swash scaler construction on glyph cache hits; moved it behind the miss
callback. Removed rounded-border temporary Vec and per-quad generator clone. The next measurement
reported zero warm CPU assembly allocations/reallocations and 0.44-0.62 ms assembly. The older
retained-Scene record was about 0.13 ms: less retained draw storage does not make rebuilding free.
Final measured results and verification are in docs/framework-draw-report.md.

Native pixel proofs check sequential overlapping effects across widgets, ordinary paint between
effects, translated backgrounds, cold regeneration, GPU repacking and failed-assembly cleanup.
CPU proofs check phase order, resource retention/pruning, nested arbitrary mask scopes and error
recovery. Existing widget invalidation tests now compare revisions. Existing SceneRenderer
contract tests remain unchanged. Deferred interface flags/policies remain deferred.

Final workspace suite: 460 passed, 0 failed, 8 existing ignored; workspace examples build passed.
Vulkan and DX12 stress runs passed all assertions. Showcase pixel diff: 0 / 900000 changed.
Final release warm assembly: 0.499 / 0.455 / 0.451 ms, zero allocations/reallocations; warm GPU
generation zero; full frame including GPU wait 4.61 / 3.40 / 3.44 ms. Not a portable FPS claim.

One rustfmt attempt hit Windows mapped-file error 1224 while compiler processes were active;
waiting for compilation to finish and formatting changed files succeeded. No filesystem deletion
or permissions workaround was needed.
