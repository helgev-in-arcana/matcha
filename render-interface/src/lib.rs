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
//! GPU callbacks may record copy/compute/render work into their dedicated output.
//! They must initialize it, never mutate the snapshot, retain borrowed handles,
//! destroy resources or submit work themselves. CPU preparation errors abort
//! submission and publication of newly generated cache entries for the frame.
//! This does not roll back a callback's external CPU side effects. Exposing
//! raw wgpu handles is a trusted extension contract, not a security sandbox.
//! A logical output need not be a fresh allocation: it can be reused after its
//! commands and placement copy have been recorded. Never retain its identity or
//! depend on previous contents. A snapshot can likewise alias the accumulation
//! image if command order puts all generation reads before that phase's writes.
//! PrepareResult reports CPU recording errors; wgpu validation/device errors use
//! wgpu's error model and are not implied absent by an Ok return.

mod prepare;
mod resource;
mod scene;
mod upload;

pub use nalgebra::Matrix4;
pub use wgpu;

pub use prepare::{
    GpuPrepareContext, MaskPrepareContext, MeshPrepareContext, MeshTarget, PrepareError,
    PrepareResult, RenderSnapshot, TexturePrepareContext, TextureTarget,
};
pub use resource::{
    DuplicateResource, MaskDescriptor, MaskId, MaskPrepare, MaskSource, MeshDescriptor, MeshId,
    MeshPrepare, MeshSource, ResourcePool, TextureDescriptor, TextureId, TexturePrepare,
    TextureSource, Vertex,
};
pub use scene::{Object, Phase, PixelMask, PixelMaskIndex, Renderer, Scene};
pub use upload::{upload_buffer, upload_texture};
