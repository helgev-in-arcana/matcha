//! GUI scene composition and renderer-owned GPU residency.
//!
//! This crate depends on the upstream render-interface contract, not on either
//! UI framework or the widget drawing helpers in the `renderer` crate.
//! Scene definitions are borrowed only during render. Internal planning, residency,
//! placement and composition types are deliberately not part of the public API.
//! CPU errors discard an unsubmitted recording. GPU errors remain on wgpu's error
//! channel; successful submission is not a GPU validation or completion receipt.
//!
//! Source images support R8Unorm, Rgba8Unorm, Rgba8UnormSrgb and Rgba16Float.
//! Destinations additionally support Bgra8Unorm/Bgra8UnormSrgb, but not R8Unorm.
//! The working colour image uses Rgba16Float; coverage uses R8Unorm. No optional
//! device features are required. Final destinations are full, single-layer/mip/
//! sample 2D views. Unsupported descriptors fail before source callbacks run.
//! AnyRegion providers generate directly into texture rectangles or aligned mesh
//! slices in shared pages. WholeResource providers use complete output resources
//! and, in atlas mode, a subsequent placement copy. Pages are separated by actual
//! usage and mesh binding role so simultaneous writable bindings do not alias.
//!
//! The dedicated-resource path is the correctness reference. Both modes use a
//! soft resident-content budget (default 128 MiB); the current frame stays pinned.
//! Reusable generation outputs have a separate retained budget (default 16 MiB).
//! Attachments, parameters and driver/in-flight storage are not charged to either.
//! A renderer must be recreated when its device is replaced;
//! providers capturing device-specific pipelines must also recreate those captures.
mod compositor;
mod frame;
mod plan;
mod resources;
mod validation;
use compositor::Compositor;
use frame::Surfaces;
use render_interface::*;
use resources::ResourceStore;
pub use resources::{
    PlacementMode,
    placement::{AtlasConfig, PlacementStats},
    relocation::RelocationStats,
};
/// Destination for the contract's premultiplied linear-light composition.
///
/// Source sampling and the working image supply premultiplied linear RGB. Final
/// output to non-sRGB Unorm or Float views keeps those linear values (subject to
/// the format's precision/range). An sRGB view hardware-encodes RGB on write;
/// alpha stays linear. This applies equally to an explicitly enabled sRGB view
/// of an Unorm texture. No transfer function is inferred from the destination's
/// role as an offscreen image or a displayed surface.
///
/// Presentation belongs to the caller. A WebGPU canvas's preferred texture
/// format is Unorm, while its `colorSpace`/`alphaMode` define how stored values
/// are displayed. Select the view and any final presentation conversion for that
/// encoding; see [WebGPU colour encoding](https://gpuweb.github.io/gpuweb/#color-spaces).
/// In particular, encoding premultiplied linear RGB does not generally produce
/// RGB premultiplied in the encoded space. An sRGB view alone therefore does not
/// adapt translucent output for a canvas expecting encoded premultiplied RGB.
/// An opaque result can use the sRGB view directly for sRGB-encoded presentation;
/// transparent presentation may need unpremultiply, encode, and re-premultiply.
/// This renderer performs no gamut, tone-mapping, or presentation-alpha conversion.
pub struct PlainTarget<'a> {
    /// Full, single-sample 2D attachment; size is taken from its texture.
    pub view: &'a wgpu::TextureView,
    /// Actual attachment view format, including any sRGB reinterpretation.
    /// wgpu exposes the underlying texture format but not its view descriptor;
    /// the caller must declare the format used to create this full 2D view.
    pub format: wgpu::TextureFormat,
    pub viewport: [f32; 2],
    /// Initial premultiplied linear-light RGBA, independent of output view format.
    pub clear: wgpu::Color,
    /// Optional full-size sampled initial image, composited over clear before
    /// phase zero. It must not alias the destination or any source output.
    pub initial: Option<&'a wgpu::TextureView>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlainError {
    #[error("invalid scene: {0}")]
    Invalid(String),
    #[error("resource {id} preparation failed: {source}")]
    Prepare { id: u64, source: PrepareError },
}
#[derive(Debug, Default, Clone, Copy)]
/// Recording counters and managed-capacity estimates. Counters may include work
/// discarded after a CPU error; they never certify GPU completion. Capacity
/// excludes driver overhead and resources retained only by in-flight submissions.
pub struct RenderStats {
    /// Successfully recorded source callbacks, including later-aborted recordings.
    pub prepared: usize,
    pub cache_hits: usize,
    pub draw_calls: usize,
    /// Render passes containing draws (excludes initial/final clears).
    pub draw_batches: usize,
    pub mask_passes: usize,
    /// Backend snapshot materializations; zero for this ordered eager backend.
    pub snapshot_copies: usize,
    /// Logical resident content, excluding page slack and temporary outputs.
    pub cache_bytes: u64,
    pub placement: PlacementStats,
    pub evicted: usize,
    /// Newly created bind groups during recording; a stable warm frame reuses them.
    pub bind_groups: usize,
    pub output_texture_allocations: usize,
    pub output_buffer_allocations: usize,
    /// Backend placement copies after logical image generation (not provider
    /// commands or explicit compaction). Direct AnyRegion preparation is zero.
    pub preparation_texture_copies: usize,
    /// Backend placement copies for vertex/index outputs, counted separately.
    pub preparation_buffer_copies: usize,
    /// Retained generation outputs, excluding the current resident content.
    pub scratch_bytes: u64,
    pub scratch_peak_bytes: u64,
    pub scratch_reuses: usize,
    /// A frame's required resident working set may exceed the soft budget.
    pub over_budget_bytes: u64,
    /// Accumulation and mask attachment capacity for the current target size.
    pub working_bytes: u64,
    /// Retained uniform arena capacity, which may exceed the current draw count.
    pub parameter_bytes: u64,
}

/// Owns the renderer's device-local resources and submission order.
pub struct PlainRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    compositor: Compositor,
    resources: ResourceStore,
    preparation: plan::FramePlan,
    surfaces: Option<Surfaces>,
    stats: RenderStats,
}
impl PlainRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            device: device.clone(),
            queue: queue.clone(),
            compositor: Compositor::new(device, queue),
            resources: ResourceStore::new(device),
            preparation: plan::FramePlan::default(),
            surfaces: None,
            stats: RenderStats::default(),
        }
    }
    pub fn stats(&self) -> RenderStats {
        self.stats
    }
    /// Drops resident content. Providers must resupply definitions on subsequent calls.
    pub fn clear_cache(&mut self) {
        self.resources.clear();
        self.compositor.clear_bind_groups();
        self.refresh_resource_stats();
    }
    /// Changes physical placement and clears resident content, leaving Scene IDs intact.
    pub fn set_placement_mode(&mut self, mode: PlacementMode) {
        self.resources.set_mode(mode);
        self.compositor.clear_bind_groups();
        self.refresh_resource_stats();
    }
    pub fn set_atlas_config(&mut self, config: AtlasConfig) -> Result<(), PlainError> {
        self.resources.set_config(config)?;
        self.compositor.clear_bind_groups();
        self.refresh_resource_stats();
        Ok(())
    }
    /// Soft logical-content budget. Eviction occurs after successful submission;
    /// all resources used by that frame remain pinned even when over budget.
    pub fn set_cache_budget(&mut self, bytes: u64) {
        self.resources.set_budget(bytes);
        self.refresh_resource_stats();
    }
    /// Repack resident content using GPU copies, without source callbacks or
    /// readback. Old and replacement capacity coexist while copies are in flight.
    /// This heuristic need not reduce capacity for every distribution of sizes.
    /// Dedicated storage has no shared-page fragmentation and is left unchanged.
    pub fn compact_resources(&mut self) -> Result<RelocationStats, PlainError> {
        self.compact_resources_with_budget(u64::MAX)
    }
    /// Declines the complete relocation before allocation/recording if its
    /// logical copy volume exceeds the limit. The old placement remains usable.
    pub fn compact_resources_with_budget(
        &mut self,
        max_copy_bytes: u64,
    ) -> Result<RelocationStats, PlainError> {
        if self.resources.placement_mode() == PlacementMode::Dedicated {
            let placement = self.resources.placement_stats();
            return Ok(RelocationStats {
                peak_managed_bytes: placement.reserved_texture_bytes
                    + placement.reserved_mesh_bytes,
                placement,
                ..Default::default()
            });
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("plain renderer transactional relocation"),
            });
        let plan = self
            .resources
            .plan_relocation(&mut encoder, max_copy_bytes)?;
        let stats = plan.stats;
        self.queue.submit([encoder.finish()]);
        self.resources.commit_relocation(plan);
        self.compositor.clear_bind_groups();
        self.refresh_resource_stats();
        Ok(stats)
    }
    /// Limits retained generation outputs; it does not reject large generators.
    pub fn set_scratch_budget(&mut self, bytes: u64) {
        self.resources.set_scratch_budget(bytes);
        self.refresh_resource_stats();
    }
    /// On a provider panic with unwinding enabled, discards unsubmitted work and
    /// new residents, restores reusable workspace, then resumes the original panic.
    /// A caller catching it can render again without clearing previously valid
    /// residents. Provider-owned side effects are not rolled back; panic=abort
    /// terminates the process and cannot run this cleanup.
    pub fn render(&mut self, scene: &Scene, target: PlainTarget<'_>) -> Result<(), PlainError> {
        self.stats = RenderStats::default();
        self.resources.begin()?;
        if let Err(error) = self.preparation.rebuild(scene).and_then(|()| {
            validation::validate(&self.device, &mut self.resources, scene, &target)?;
            Ok(())
        }) {
            self.refresh_resource_stats();
            return Err(error);
        }
        let size = [
            target.view.texture().width(),
            target.view.texture().height(),
        ];
        if self.surfaces.as_ref().is_none_or(|s| s.size != size) {
            self.surfaces = Some(Surfaces::new(&self.device, size));
        }
        let result = frame::encode(
            &self.device,
            &mut self.compositor,
            &mut self.resources,
            scene,
            &self.preparation,
            target,
            self.surfaces.as_ref().expect("surfaces were initialized"),
            &mut self.stats,
        );
        match result {
            Ok(frame) => {
                // Queue writes occur only after every fallible CPU callback. The queue
                // orders this upload before the command buffer's uniform reads.
                self.queue
                    .write_buffer(&frame.uniforms, 0, &frame.storage.bytes);
                self.queue.submit([frame.encoder.finish()]);
                self.compositor.restore_workspace(frame.storage);
                self.resources.finish(scene);
            }
            Err(failure) => {
                self.resources.abort();
                self.refresh_resource_stats();
                return match failure {
                    frame::FrameFailure::Recording(error) => Err(error),
                    frame::FrameFailure::Unwind(payload) => std::panic::resume_unwind(payload),
                };
            }
        }
        self.refresh_resource_stats();
        Ok(())
    }
    fn refresh_resource_stats(&mut self) {
        self.stats.prepared = self.resources.stats.prepared;
        self.stats.cache_hits = self.resources.stats.cache_hits;
        self.stats.output_buffer_allocations = self.resources.stats.output_buffer_allocations;
        self.stats.output_texture_allocations = self.resources.stats.output_texture_allocations;
        self.stats.preparation_texture_copies = self.resources.stats.preparation_texture_copies;
        self.stats.preparation_buffer_copies = self.resources.stats.preparation_buffer_copies;
        let scratch = self.resources.scratch_stats();
        self.stats.output_buffer_allocations += scratch.buffer_allocations;
        self.stats.output_texture_allocations += scratch.image_allocations;
        self.stats.scratch_bytes = scratch.pooled_bytes;
        self.stats.scratch_peak_bytes = scratch.peak_bytes;
        self.stats.scratch_reuses = scratch.image_reuses + scratch.buffer_reuses;
        self.stats.evicted = self.resources.stats.evicted;
        self.stats.over_budget_bytes = self.resources.over_budget_bytes();
        self.stats.cache_bytes = self.resources.cache_bytes();
        self.stats.placement = self.resources.placement_stats();
        self.stats.working_bytes = self
            .surfaces
            .as_ref()
            .map_or(0, |s| u64::from(s.size[0]) * u64::from(s.size[1]) * 14);
        self.stats.parameter_bytes = self
            .compositor
            .parameter_buffer
            .as_ref()
            .map_or(0, wgpu::Buffer::size);
    }
}
impl Renderer for PlainRenderer {
    type Target<'a> = PlainTarget<'a>;
    type Error = PlainError;
    fn render(&mut self, scene: &Scene, target: PlainTarget<'_>) -> Result<(), PlainError> {
        PlainRenderer::render(self, scene, target)
    }
}
