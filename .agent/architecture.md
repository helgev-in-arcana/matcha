# Architecture — `matcha-ecs` core

The framework core. Rendering contracts live upstream in render-interface; widgets own native local Scenes. Read this before touching anything in `matcha-ecs/src/`, and before writing a
widget.

## Rendering ownership direction (review 2026-09-23)

The current implementation still retains widget-local Scenes. The agreed direction is to remove
widget ownership of Scene phases: the framework must resolve paint/backdrop semantics into final
phases. Neither zipping local phases nor concatenating whole widget Scenes is generally correct.
Widgets should supply draw content and reusable resources through lightweight construction.
Do not introduce Scene clones or per-Object Arc/reference wrappers as the default optimization;
compare lightweight Object construction with retention on representative widgets first. Preserve
content IDs across unchanged outputs so CPU reconstruction does not imply GPU regeneration.
See ../docs/render-interface-review-notes.md for decisions and measurement criteria. This direction
is documented, not yet implemented; the module map below describes the current code.

## The one dependency rule

`matcha-ecs-widgets` → `matcha-ecs`, **never the reverse**. The core names no widget, no layout
type, no text engine, no animation. When something must be shared between widgets (a click message,
a focus policy, an opacity value), it goes into the core as a **neutral protocol** — a component the
core reads without knowing who writes it — not as the feature itself.

The core does name `matcha-window` types (`KeyInput`, `ImeEvent`, `CursorIcon`). That is deliberate;
`components/input.rs`'s module docs argue it out, including why the argument used to be weaker than
it looked (until `d787f2f` those key types were a straight re-export of winit's).

## Module map — `matcha-ecs/src/`

Every one of these has real `//!` docs. Read the module, not a summary of it.

| File | Owns |
|---|---|
| `ui_ecs.rs` | `UiEcs<M, Msg, F, R>` — the `Application` driver: world, schedules, window/surface lifecycle, event entry points, the builder (`with_*`) surface |
| `view.rs` | `Widget` trait, `Scope`, reconciliation. **Do not change these semantics casually** |
| `layout.rs` | `Constraints`, `Measured`, `Layout`, `LayoutDispatch`, `LayoutCtx`, `layout_root`/`run_layout` |
| `scene.rs` | Flat Scene composition, shared source imports, mask validation/rebasing |
| `render.rs` | Extract → `RenderSnapshot` → `RenderDriver` (`ThreadDriver` default, `InlineDriver` fallback and web) |
| `clip.rs` | `Clip` markers → the renderer's clip arena. GPU-free by design |
| `traversal.rs` | **The one order** painting and picking both walk, plus `ZIndex` stacking |
| `pick.rs` | `Picker` trait + `RectZPicker`. Contract: picking returns **at most one entity** |
| `input.rs` | Click routing (bubbling), `PointerCapture`, `MessageQueue<Msg>` |
| `pointer.rs` | `:hover` / `:active` resolution over the pick chain |
| `focus.rs` | Focus path resolution (upward walk + policy pass) |
| `tab_order.rs` | Sequential focus navigation over `traversal`'s order |
| `keyboard.rs` | Key/IME delivery down the focus path; IME state sync to the window |
| `clipboard.rs` | System clipboard as a lazily-opened world resource |
| `task.rs` | Entity-lifetime-bound async tasks (`bevy_tasks`) |
| `model.rs` | `ModelHandle<M>` — the write side of the model loop |
| `systems.rs`, `resources.rs`, `components/` | Framework systems, world resources, the component protocols |

## The frame

Two schedules. The render schedule runs four sets, in order:

```
MatchaSet::PreLayout    open — settle everything layout reads, and the tree's shape
MatchaSet::Layout       core — measure/arrange, writes LayoutOutput + GlobalTransform
MatchaSet::PreExtract   open — settle the extract contract now that layout is known
MatchaSet::Extract      core — collect drawable entities into a RenderSnapshot
```

`PreLayout` and `PreExtract` are **timing contracts, not feature buckets**. That is the point of the
naming: a system registered in `PreLayout` is promising to be done before anything measures. The
core registers nothing in `PreLayout` and only plumbing in `PreExtract` (invalidation, picker
update, focus sync). Apps and widget crates register via `UiEcs::with_pre_layout_systems` /
`with_pre_extract_systems`; no ordering is imposed between separately-registered systems, and none
is forbidden.

Then: acquire the surface texture on the main thread, extract, hand the snapshot to the
`RenderDriver`, which builds or updates native widget Scenes, assembles a borrowed window Scene, records GPU work and presents (on a worker thread by default).

## Event → pixels

```
window/device event
  → pick (one entity)            pick.rs
  → walk up                      traversal::ancestors
      → click target (bubbling)  input.rs      → Msg → reducer → model
      → focus path               focus.rs      → key/IME delivery
      → hover/active chain       pointer.rs
  → re-run the view fn           view.rs (reconcile against the existing entities)
  → request redraw
```

A **focus-only or hover-only change redraws but does not re-run the view** — that state is ECS
state, not model state.

`Msg` reaches the model two ways: an `OnClick<Msg>` read at dispatch time, or `MessageQueue<Msg>`
(`input::emit_message`) for handlers that run behind a non-generic fn pointer and cannot reach the
reducer themselves — which is every keyboard/IME/pointer handler.

## Writing a widget

A widget is a plain value implementing `Widget`. Reconciliation matches by `TypeId` + key; a
type change rebuilds the entity.

```rust
fn bundle(&self) -> impl Bundle      // components spawned once. Fixed type — no conditionals
fn after_spawn(&self, e: &mut EntityWorldMut)   // anything needing world/resource access
fn patch(&self, e: &mut EntityWorldMut)         // sync from a re-declared value
```

Rules that are easy to get wrong:

- **`bundle()` returns one fixed type.** `Option<T>` is not a `Bundle`. Anything conditional
  (a marker like `Clip`, a tween, a component that depends on a resource) is inserted in
  `after_spawn`/`patch` instead.
- **`patch` should `set_if_neq`** so a no-op re-declare does not invalidate a cached Scene.
  Exception: fn-pointer fields, where comparison is meaningless — assign them outright.
- **Widgets are declarative.** The app passes current state every `view()` call and the widget holds
  none. `TextBox` is the single, necessary exception (see [text.md](text.md)).
- Layout is wired by including `(XxxLayout, LayoutDispatch::of::<XxxLayout>())` in the bundle. There
  is **no registration step and no registry.**
- A widget that draws carries a `RenderItem`: a *builder closure* plus a shared cache slot.
  `RenderItem::invalidate()` swaps the cache so the next frame rebuilds.

## How a value reaches a `RenderItem` builder

This constrains more designs than anything else in the codebase. The builder is a closure captured
at `bundle()`/`patch()` time; it runs **on the render thread, with no `World` and no `&self`**.
There are exactly three routes:

1. **A `RenderCtx` field** — for what the core knows: `size`, `focused`, `focus_within`,
   `hovered`, `active`. Free, no invalidation.
2. **A shared cell the closure captured** (`matcha-ecs-widgets/src/live.rs`) — for what the core
   does not: a wrap width settled by `arrange`, a scroll offset, a caret blink phase. Cheap.
3. **Rebuilding the closure** — costs an invalidation and a CPU re-rasterisation. Only for things that
   genuinely change what is drawn.

Corollary: **a widget cannot read focus, hover or world state at paint time.** If you find yourself
wanting to, the answer is route 1 or 2.

## Contracts worth stating once

- **A widget's declared size is a layout *input*** (what its `measure` reports). Paint always tracks
  the **allocated** size (`RenderCtx::size`, from `LayoutOutput`), which is also what hit-testing
  and child arrangement use. A builder must never bake in its constructor's `w`/`h`.
- **`Changed<T>` does not fire on component removal.** An invalidation system watching
  `Changed<Focused>` leaves an entity *losing* focus painting its ring forever. Transitions are
  handled where both directions are known (e.g. `focus::sync_focus_components`), and widgets should
  prefer `Has<Focused>` / the `Focus` resource.
- **`ManualDespawn`** opts an entity out of the reconciler's prune-time despawn: the reconciler only
  flags it pruned and keeps the slot (so it still lays out, paints and hit-tests); despawning is the
  registered system's job, via `view::despawn_ui_entity`. Revival clears the flag *after* `patch`
  runs, so `patch` can reverse an in-flight exit animation. **No despawning system ⇒ a leak.**
- **Dropping a `bevy_tasks::Task` stops it being polled, but does not promptly reclaim what the
  future captured** — confirmed experimentally, not read from docs. A task needing prompt cleanup
  must check a cooperative cancellation flag itself. See `task.rs`'s docs.

## Rendering boundary

RenderItem::new builds a retained native Scene until invalidated. RenderItem::dynamic mutates its
retained Scene on each redraw, permitting fresh IDs for backdrop/time-dependent content without
reallocating all drawing arrays. The cache owns Scene directly inside its mutex; no Arc<Scene>
wrapper or paint tree. Snapshot extraction shares only the cache slot and builder.

RenderCtx has CPU layout/interaction state plus resolved transform and viewport_size. It has no
Device/Queue fields; source preparation receives native wgpu recording contexts. Providers may
cache private GPU programs. Final texture/mesh placement remains renderer-owned.

GuiRenderer::assemble validates/rebases local mask indices, applies placement/opacity and imports
source definitions, including unused retention candidates. Dead definitions are pruned on failed
assemblies too. GuiRenderer::render_extracted adds the backend call; offscreen proofs use the same
assembly path. The GuiRenderer mutex serializes shared-window assembly/recording.

ClipReset in components/layout.rs resets the inherited clipping chain in clip::descend. Both
picking and extraction call that function. It does not change placement or ZIndex. Arbitrary Scene
phases can change paint order beyond the default UI traversal; applications must align picking
policy when exploiting that freedom.
