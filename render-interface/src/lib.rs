//! The upstream rendering contract, independent of Matcha's UI and backend.
//!
//! Design constraints for optional extensions outside this contract:
//! - A snapshot-dependency declaration needs a defined relationship between
//!   immutable content IDs and the phase-start image. Cache invalidation and
//!   regeneration remain renderer responsibilities.
//! - A sampler policy needs explicit filtering/addressing semantics and defaults,
//!   independent of physical atlas placement.
//! - Additional shader/draw settings, such as blend policy, need a defined scope
//!   and composition semantics. These extension notes add no fields or behavior.
//!
//! A caller owns and may reuse a complete [`Scene`]. A renderer borrows it only
//! during `render`; CPU callbacks finish before that call returns, GPU work need
//! not. Resource pools store Source values. IDs identify immutable logical
//! content, never GPU placement, revisions or drawing order.
//! A changed animation/background input requires a new ID. Regeneration with an
//! existing ID must reproduce its content, regardless of cache eviction.
//! Each Source holds its immutable generator in an `Arc<Prepare>`. Source::clone
//! shares that allocation between provider caches and submitted resource pools.
//! Explicit pool composition deduplicates IDs; direct duplicate insertions remain
//! errors. Closure pointer equality is not a substitute for logical content identity.
//!
//! Phases and objects are composited in array order, using premultiplied linear
//! RGBA and source-over. Vertex positions are object-local, Y-down; transforms
//! map them to logical Scene coordinates. UVs are normalized, clamped and linearly
//! filtered. The fixed vertex ABI gives producers and renderers shared meanings
//! for position and texture coordinates.
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
