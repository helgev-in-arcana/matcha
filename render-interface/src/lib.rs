//! The upstream rendering contract, independent of Matcha's UI and backend.
//!
//! Deferred extension candidates (not adopted or implemented):
//! - Snapshot-dependency flag: let a source declare that it reads the snapshot;
//!   leave invalidation/regeneration policy to the renderer. The relation to
//!   immutable content IDs and snapshot versions still needs a contract.
//! - Sampler policy: explicit sampling semantics, notably nearest versus linear,
//!   independent of atlas placement. Fields and defaults remain undecided.
//! - Other shader/draw settings beyond sampler policy, such as blend policy:
//!   the useful settings, their scope and their representation remain undecided.
//!
//! These notes do not change the current contract described below.
//!
//! A caller owns and may reuse a complete [`Scene`]. A renderer borrows it only
//! during `render`; CPU callbacks finish before that call returns, GPU work need
//! not. Sources are stored directly, not behind an additional Arc. IDs identify
//! immutable logical content, never GPU placement, revisions or drawing order.
//! A changed animation/background input requires a new ID. Regeneration with an
//! existing ID must reproduce its content, regardless of cache eviction.
//! Source::clone shares the immutable generator allocation (`Arc<Prepare>` in place
//! of `Box<Prepare>`) so provider caches and the submitted pool can share definitions.
//! Explicit pool composition deduplicates IDs; direct duplicate insertions remain
//! errors. Closure pointer equality is not a substitute for logical content identity.
//!
//! Phases and objects are composited in array order, using premultiplied linear
//! RGBA and source-over. Vertex positions are object-local, Y-down; transforms
//! map them to viewport UI pixels. UVs are normalized, clamped and linearly filtered.
//! The fixed vertex ABI is intentional: arbitrary byte layouts without shared
//! attribute semantics do not constitute an interoperable rendering interface.
//!
//! Masks have arbitrary meshes and coverage textures (red, linear `[0,1]`). Only
//! coverage is inherited from their parent, by multiplication; transforms are
//! absolute. Overlapping triangles of one mask take maximum coverage. No depth
//! testing, G-buffer, previous-frame history or implicit UI ancestry is promised.
//!
//! A source is prepared on its first referenced phase, including references
//! through mask ancestors. All preparations in a phase read its *start* image,
//! never earlier objects of that phase. Registration alone requests no work.
//! Definitions must exist even on a warm cache. Caches may ignore pool retention
//! hints, but must not regenerate a source from a later phase's snapshot.
//!
//! GPU callbacks record copy/compute/render work into their logical output. The
//! default [`PrepareOutputLayout::WholeResource`] guarantees a whole allocation;
//! generators declaring [`PrepareOutputLayout::AnyRegion`] also accept texture
//! rectangles or buffer slices inside larger allocations. They initialize only
//! that output, preserving neighbouring data. Snapshot readers always honor its
//! region, independently of output layout. Renderer placement remains private.
//! Callbacks must never mutate the snapshot, retain output/snapshot
//! handles, destroy borrowed resources or submit work themselves. Device clones
//! and privately created pipelines/work resources may be retained by providers.
//! CPU preparation errors abort
//! submission and publication of newly generated cache entries for the frame.
//! This does not roll back a callback's external CPU side effects. Exposing
//! raw wgpu handles is a trusted extension contract, not a security sandbox.
//! [`TextureRegion::begin_render_pass`] sets viewport/scissor and implements
//! region-local colour Clear. Its raw pass can still override these settings.
//! Storage shaders must offset and bound their own writes. A renderer must
//! prevent incompatible read/write use of the same physical subresource; pixel
//! rectangles do not create independent GPU usage scopes. Failure fallback is a
//! placement decision before invoking a callback, not a retry after its error.
//! A logical output need not be a fresh allocation: it can be reused after its
//! commands and placement copy have been recorded. Never retain its identity or
//! depend on previous contents. A snapshot can likewise alias the accumulation
//! image if command order puts all generation reads before that phase's writes.
//! PrepareResult reports CPU recording errors; wgpu validation/device errors use
//! wgpu's error model and are not implied absent by an Ok return.
//!
//! The optional `scene-builder` feature additionally exports `Frame`, `Draw`,
//! `unit_quad` and `push_quad` for CPU scene assembly. It is disabled by default;
//! callers may construct `Scene` directly and choose their own scheduling and
//! definition-retention policy.

mod interface;
#[cfg(feature = "scene-builder")]
mod scene_builder;

pub use interface::*;
#[cfg(feature = "scene-builder")]
pub use scene_builder::*;
