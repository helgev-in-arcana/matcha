# 2026-09-23: framework owns phase semantics, resource reuse is separate

The user clarified that neither interleaving local phases nor concatenating local Scenes has
enough information to select correct cross-widget ordering. Withdrew local Scene/Phase ownership
as the future widget API direction. Framework scheduling must be based on defined paint/backdrop
semantics; widgets contribute draw content/resources, not phase indices. Private provider GPU
passes are separate from this global ordering problem.

Recorded the preference to investigate lightweight Object-producing closures before adding
Clone/Arc/borrowed Object retention. Distinguished CPU builder cost from GPU Source preparation;
stable content IDs permit GPU reuse even when draw records are constructed again. Heavy shaping
or decoding must remain outside the lightweight assembly step. Performance is not inferred from
ownership syntax or the previous warm-frame measurement; representative comparison criteria are
in docs/render-interface-review-notes.md. No runtime or public API change in this discussion.

Async wgpu errors are assigned to application/event-loop handling, not an interface-level
synchronous-success guarantee. Existing native validation timing evidence remains valid but is
not a universal WebGPU error delivery model. Documentation-only change; no tests rerun.
