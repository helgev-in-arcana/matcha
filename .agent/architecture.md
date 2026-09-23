# Architecture — `matcha-ecs` core

The framework core. Rendering contracts live upstream in render-interface; the framework owns the final Scene and widgets write Objects. Read this before touching anything in `matcha-ecs/src/`, and before writing a
widget.

## Rendering ownership (2026-09-23)

RenderItem writers receive RenderCtx and Draw, never a local Scene or phases. Frame owns the
reusable final Scene and resource pool. Draw::backdrop means "read preceding paint"; Frame
inserts a phase boundary before that Object. Ordinary objects retain UI traversal order.
No per-Object Arc or Scene cache exists. A single builder Arc per entity is shared with extracted
frames; invalidation advances a value revision without allocating. Providers retain expensive
shaped layouts, decoded images and native resource definitions with stable content IDs.

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
| `scene.rs` | Frame/Draw writers, framework phase scheduling, resource registration and mask scopes |
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
`RenderDriver`, which invokes draw writers into a reusable window Scene, records GPU work and presents (on a worker thread by default).

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
- **`patch` should `set_if_neq`** so a no-op re-declare does not advance the draw revision.
  Exception: fn-pointer fields, where comparison is meaningless — assign them outright.
- **Widgets are declarative.** The app passes current state every `view()` call and the widget holds
  none. `TextBox` is the single, necessary exception (see [text.md](text.md)).
- Layout is wired by including `(XxxLayout, LayoutDispatch::of::<XxxLayout>())` in the bundle. There
  is **no registration step and no registry.**
- A widget that draws carries a `RenderItem`: a shared writer plus a value revision.
  `RenderItem::invalidate()` advances the revision; each redraw invokes the writer.

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

RenderItem::new accepts Fn(&RenderCtx, &mut Draw). Writers emit Objects and masks directly into
reusable frame storage every redraw. Shaping and decoding caches belong to providers. Source
registration retains existing definitions on matching IDs, and finish prunes definitions absent
from this submission even after assembly failure. See scene.rs for scheduling/ownership docs.

RenderCtx contains resolved placement/viewport and interaction state, no Device or Queue. Native
GPU preparation remains in Source callbacks. Draw scopes apply geometry, inherited masks and
opacity exactly once. ClipReset resets inherited clips for both extraction and picking; Draw
does not expose phase indices that could reorder objects independently of picking.
