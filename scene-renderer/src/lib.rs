//! GUI scene composition and renderer-owned GPU residency.
//!
//! This crate depends on the upstream render-interface contract, not on either
//! UI framework or the widget drawing helpers in the legacy renderer crate.
//! Scene definitions are borrowed only during render. Internal planning, residency,
//! placement and composition types are deliberately not part of the public API.
//! CPU errors discard an unsubmitted recording. GPU errors remain on wgpu's error
//! channel; successful submission is not a GPU validation or completion receipt.
//!
//! Source images support R8Unorm, Rgba8Unorm, Rgba8UnormSrgb and Rgba16Float.
//! Destinations additionally support Bgra8Unorm/Bgra8UnormSrgb, but not R8Unorm.
//! The working colour image uses Rgba16Float; coverage uses R8Unorm. No optional
//! device features are required. All targets are full, single-layer/mip/sample
//! 2D views. Unsupported descriptors fail before source callbacks run.
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
pub struct SceneTarget<'a> {
    /// Full, single-sample 2D attachment; size is taken from its texture.
    pub view: &'a wgpu::TextureView,
    /// Actual attachment view format, including any sRGB reinterpretation.
    /// wgpu exposes the underlying texture format but not its view descriptor;
    /// the caller must declare the format used to create this full 2D view.
    pub format: wgpu::TextureFormat,
    pub viewport: [f32; 2],
    pub clear: wgpu::Color,
    /// Optional full-size sampled initial image, composited over clear before
    /// phase zero. It must not alias the destination or any source output.
    pub initial: Option<&'a wgpu::TextureView>,
}

#[derive(Debug, thiserror::Error)]
pub enum SceneError {
    #[error("invalid scene: {0}")]
    Invalid(String),
    #[error("resource {id} preparation failed: {source}")]
    Prepare { id: u64, source: PrepareError },
}
#[derive(Debug, Default, Clone, Copy)]
pub struct RenderStats {
    pub prepared: usize,
    pub cache_hits: usize,
    pub draw_calls: usize,
    /// Render passes containing draws (excludes initial/final clears).
    pub draw_batches: usize,
    pub mask_passes: usize,
    /// Backend snapshot materializations; zero for this ordered eager backend.
    pub snapshot_copies: usize,
    pub cache_bytes: u64,
    pub placement: PlacementStats,
    pub evicted: usize,
    pub bind_groups: usize,
    pub output_texture_allocations: usize,
    pub output_buffer_allocations: usize,
    /// Retained generation outputs, excluding the current resident content.
    pub scratch_bytes: u64,
    pub scratch_peak_bytes: u64,
    pub scratch_reuses: usize,
    /// A frame's required resident working set may exceed the soft budget.
    pub over_budget_bytes: u64,
    pub working_bytes: u64,
    pub parameter_bytes: u64,
}

/// Owns the renderer's device-local resources and submission order.
pub struct SceneRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    compositor: Compositor,
    resources: ResourceStore,
    surfaces: Option<Surfaces>,
    stats: RenderStats,
}
impl SceneRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            device: device.clone(),
            queue: queue.clone(),
            compositor: Compositor::new(device, queue),
            resources: ResourceStore::new(device),
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
        self.refresh_resource_stats();
    }
    /// Changes physical placement and clears resident content, leaving Scene IDs intact.
    pub fn set_placement_mode(&mut self, mode: PlacementMode) {
        self.resources.set_mode(mode);
        self.refresh_resource_stats();
    }
    pub fn set_atlas_config(&mut self, config: AtlasConfig) -> Result<(), SceneError> {
        self.resources.set_config(config)?;
        self.refresh_resource_stats();
        Ok(())
    }
    /// Soft logical-content budget. Eviction occurs after successful submission;
    /// all resources used by that frame remain pinned even when over budget.
    pub fn set_cache_budget(&mut self, bytes: u64) {
        self.resources.set_budget(bytes);
    }
    /// Repack resident content using GPU copies, without source callbacks or
    /// readback. Old and replacement capacity coexist while copies are in flight.
    /// This heuristic need not reduce capacity for every distribution of sizes.
    pub fn compact_resources(&mut self) -> Result<RelocationStats, SceneError> {
        self.compact_resources_with_budget(u64::MAX)
    }
    /// Declines the complete relocation before allocation/recording if its
    /// logical copy volume exceeds the limit. The old placement remains usable.
    pub fn compact_resources_with_budget(
        &mut self,
        max_copy_bytes: u64,
    ) -> Result<RelocationStats, SceneError> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("transactional scene relocation"),
            });
        let plan = self
            .resources
            .plan_relocation(&mut encoder, max_copy_bytes)?;
        let stats = plan.stats;
        self.queue.submit([encoder.finish()]);
        self.resources.commit_relocation(plan);
        self.refresh_resource_stats();
        Ok(stats)
    }
    /// Limits retained generation outputs; it does not reject large generators.
    pub fn set_scratch_budget(&mut self, bytes: u64) {
        self.resources.set_scratch_budget(bytes);
        self.refresh_resource_stats();
    }
    pub fn render(&mut self, scene: &Scene, target: SceneTarget<'_>) -> Result<(), SceneError> {
        self.stats = RenderStats::default();
        self.resources.begin()?;
        let plan = match plan::FramePlan::build(scene).and_then(|plan| {
            validation::validate(&self.device, &self.resources, scene, &target)?;
            Ok(plan)
        }) {
            Ok(plan) => plan,
            Err(error) => {
                self.refresh_resource_stats();
                return Err(error);
            }
        };
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
            &plan,
            target,
            self.surfaces.as_ref().expect("surfaces were initialized"),
            &mut self.stats,
        );
        match result {
            Ok(frame) => {
                // Queue writes occur only after every fallible CPU callback. The queue
                // orders this upload before the command buffer's uniform reads.
                self.queue.write_buffer(&frame.uniforms, 0, &frame.bytes);
                self.queue.submit([frame.encoder.finish()]);
                self.compositor.parameter_bytes = frame.bytes;
                self.resources.finish(scene);
            }
            Err(error) => {
                self.resources.abort();
                self.refresh_resource_stats();
                return Err(error);
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
impl Renderer for SceneRenderer {
    type Target<'a> = SceneTarget<'a>;
    type Error = SceneError;
    fn render(&mut self, scene: &Scene, target: SceneTarget<'_>) -> Result<(), SceneError> {
        SceneRenderer::render(self, scene, target)
    }
}
