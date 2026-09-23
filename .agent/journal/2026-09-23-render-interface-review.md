# 2026-09-23: review scope and validation timing correction

The user deferred snapshot dependency, sampler/shader policy, placement metrics and upstream
cache lifetime; the latter two are implementation topics outside the core interface. Recorded
the proposed snapshot-dependency flag without implementing it. Same resource IDs do not imply
the same rendered background when placement, draw order, masks or initial content change.

Investigated native wgpu/wgpu-core 29.0.4 locally (no GitHub operations). Refuted an overbroad
inference in the previous report: the incompatible-format Copy diagnostic does not require
waiting for GPU execution. Copy records first; encoder.finish encodes/validates on CPU and
delivers the error via a scope/handler. Native scope.pop returns ready(scope.error). The API
does not return a Result for copy/finish; gpu-utils replaces default panic handling with logging.

Added a diagnostic that observes 0 handler calls after Copy and 1 immediately after finish,
without submitting or polling. Both this and the original CPU-Ok/error-scope diagnostic passed
on Vulkan and DX12 (2 tests per backend). Production behavior/API unchanged. Corrected the
report's claim that an async receipt was needed; validation policy belongs first in the renderer.

Documented local Scene composition: separate CPU ownership, global phase alignment/snapshots,
mask index rebasing, placement composition, per-object opacity, shared definitions. It is not
an isolated compositing group. Arc<Prepare> is one framework ownership choice, not an inherent
renderer requirement. Details and deferred decisions: docs/render-interface-review-notes.md.

Workspace-wide fmt check finds existing formatting differences in legacy/unmodified files;
the two Rust files touched here pass their targeted rustfmt check. No broad reformat performed.
