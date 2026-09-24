//! Bounded reuse of dedicated logical source outputs.
//!
//! Matching images require the exact logical descriptor, including additional
//! usages. Matching buffers require exact byte size and effective usages. Taking
//! removes an output from the pool, so simultaneous vertex/index outputs cannot
//! alias even when their descriptors match. Return only after commands using
//! the output and its resident-placement copy have been recorded in order.
//! This module does not submit, wait for GPU completion, or establish that order.
//!
//! The budget limits retained outputs, not outputs currently needed by sources.
//! Older returned outputs are discarded first when newer outputs need room.
//! Pool entries are removed entirely on reuse/eviction; historical descriptors
//! do not leave empty per-size buckets. Capacity is an estimate of logical GPU
//! storage, excluding driver overhead and handles retained by submitted work.
//!
//! A recording abort may drop a borrowed output without returning it. After
//! dropping that recording and its outputs, call reset to reconcile accounting.
//! A late or duplicate return is rejected and cannot enter the pool twice.

use std::collections::HashMap;

use render_interface::{TextureDescriptor, wgpu};

use super::{Image, make_image};
use crate::SceneError;

const DEFAULT_RETAINED_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScratchStats {
    pub(crate) image_allocations: usize,
    pub(crate) buffer_allocations: usize,
    pub(crate) image_reuses: usize,
    pub(crate) buffer_reuses: usize,
    pub(crate) pooled_bytes: u64,
    pub(crate) checked_out_bytes: u64,
    /// Maximum checked-out plus pooled storage during the current frame.
    pub(crate) peak_bytes: u64,
}

enum Output {
    Image(Image),
    Buffer(wgpu::Buffer),
}

impl Output {
    fn bytes(&self) -> u64 {
        match self {
            Self::Image(image) => image.bytes(),
            Self::Buffer(buffer) => buffer.size(),
        }
    }
}

pub(crate) struct ScratchPool {
    budget: u64,
    /// Oldest returned first. Outputs are normally few and preparation-only;
    /// a dense list avoids maintaining a second cache index and empty buckets.
    pooled: Vec<Output>,
    image_loans: HashMap<wgpu::Texture, (TextureDescriptor, u64)>,
    buffer_loans: HashMap<wgpu::Buffer, u64>,
    stats: ScratchStats,
}

impl Default for ScratchPool {
    fn default() -> Self {
        Self::new()
    }
}

impl ScratchPool {
    pub(crate) fn new() -> Self {
        Self::with_budget(DEFAULT_RETAINED_BYTES)
    }

    /// A zero budget disables retention while preserving exclusive checkouts.
    pub(crate) fn with_budget(retained_bytes: u64) -> Self {
        Self {
            budget: retained_bytes,
            pooled: Vec::new(),
            image_loans: HashMap::new(),
            buffer_loans: HashMap::new(),
            stats: ScratchStats::default(),
        }
    }

    /// Start per-frame counters while preserving bounded reusable outputs.
    /// An unreturned output requires explicit reset after the previous recording
    /// is abandoned; silently zeroing its accounting could hide a lifetime bug.
    pub(crate) fn begin_frame(&mut self) -> Result<(), SceneError> {
        if !self.image_loans.is_empty() || !self.buffer_loans.is_empty() {
            return Err(SceneError::Invalid(
                "scratch outputs remain checked out; return them or reset after abort".into(),
            ));
        }
        self.stats = ScratchStats {
            pooled_bytes: self.stats.pooled_bytes,
            peak_bytes: self.stats.pooled_bytes,
            ..Default::default()
        };
        Ok(())
    }

    /// Forget all outputs and pending checkouts after their owning recording is
    /// discarded. Frame counters and peak remain available for diagnostics;
    /// begin_frame resets them. Late returns are rejected by handle identity.
    pub(crate) fn reset(&mut self) {
        self.pooled.clear();
        self.image_loans.clear();
        self.buffer_loans.clear();
        self.stats.pooled_bytes = 0;
        self.stats.checked_out_bytes = 0;
    }

    pub(crate) fn set_budget(&mut self, retained_bytes: u64) {
        self.budget = retained_bytes;
        while self.stats.pooled_bytes > self.budget {
            self.discard_oldest();
        }
    }

    pub(crate) fn stats(&self) -> ScratchStats {
        self.stats
    }

    /// Descriptor validation and use of one device for this pool belong to its
    /// owner. make_image supplies a dedicated output with no resident lease.
    pub(crate) fn take_image(&mut self, device: &wgpu::Device, desc: TextureDescriptor) -> Image {
        let matching = self
            .pooled
            .iter()
            .rposition(|entry| matches!(entry, Output::Image(image) if image.desc == desc));
        let image = if let Some(index) = matching {
            let Output::Image(image) = self.pooled.remove(index) else {
                unreachable!("the matching entry was an image")
            };
            self.stats.pooled_bytes -= image.bytes();
            self.stats.image_reuses += 1;
            image
        } else {
            self.stats.image_allocations += 1;
            make_image(device, desc)
        };
        let bytes = image.bytes();
        let previous = self
            .image_loans
            .insert(image.texture.clone(), (desc, bytes));
        debug_assert!(previous.is_none(), "an image cannot be checked out twice");
        self.checkout(bytes);
        image
    }

    /// Returns whether this exact checkout was retained for future reuse.
    /// False also covers a changed descriptor or an unknown/duplicate return.
    pub(crate) fn return_image(&mut self, image: Image) -> bool {
        let Some((desc, bytes)) = self.image_loans.remove(&image.texture) else {
            return false;
        };
        self.stats.checked_out_bytes -= bytes;
        if image.desc != desc
            || image.uv != [0., 0., 1., 1.]
            || image.view.texture() != &image.texture
            || image.texture_lease.is_some()
        {
            return false;
        }
        self.retain(Output::Image(image))
    }

    /// Size is exact; callers provide the alignment required by their commands.
    /// Source output usage always includes placement-copy source/destination.
    pub(crate) fn take_buffer(
        &mut self,
        device: &wgpu::Device,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        let usage = usage | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
        let matching = self.pooled.iter().rposition(|entry| {
            matches!(entry, Output::Buffer(buffer) if buffer.size() == size && buffer.usage() == usage)
        });
        let buffer = if let Some(index) = matching {
            let Output::Buffer(buffer) = self.pooled.remove(index) else {
                unreachable!("the matching entry was a buffer")
            };
            self.stats.pooled_bytes -= buffer.size();
            self.stats.buffer_reuses += 1;
            buffer
        } else {
            self.stats.buffer_allocations += 1;
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("scene logical scratch output"),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let previous = self.buffer_loans.insert(buffer.clone(), size);
        debug_assert!(previous.is_none(), "a buffer cannot be checked out twice");
        self.checkout(size);
        buffer
    }

    pub(crate) fn return_buffer(&mut self, buffer: wgpu::Buffer) -> bool {
        let Some(bytes) = self.buffer_loans.remove(&buffer) else {
            return false;
        };
        self.stats.checked_out_bytes -= bytes;
        self.retain(Output::Buffer(buffer))
    }

    fn checkout(&mut self, bytes: u64) {
        self.stats.checked_out_bytes = self
            .stats
            .checked_out_bytes
            .checked_add(bytes)
            .expect("live scratch storage fits in the addressable byte range");
        self.stats.peak_bytes = self.stats.peak_bytes.max(
            self.stats
                .checked_out_bytes
                .checked_add(self.stats.pooled_bytes)
                .expect("combined scratch storage fits in the addressable byte range"),
        );
    }

    fn retain(&mut self, output: Output) -> bool {
        let bytes = output.bytes();
        // Zero-size resources must not create an unbounded collection of free
        // entries that can evade the byte budget.
        if bytes == 0 || bytes > self.budget {
            return false;
        }
        while bytes > self.budget - self.stats.pooled_bytes {
            self.discard_oldest();
        }
        self.stats.pooled_bytes += bytes;
        self.pooled.push(output);
        true
    }

    fn discard_oldest(&mut self) {
        let output = self.pooled.remove(0);
        self.stats.pooled_bytes -= output.bytes();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> wgpu::Device {
        futures::executor::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::NOOP,
                backend_options: wgpu::BackendOptions {
                    noop: wgpu::NoopBackendOptions { enable: true },
                    ..Default::default()
                },
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance
                .request_adapter(&Default::default())
                .await
                .expect("noop adapter exists without a GPU");
            adapter
                .request_device(&Default::default())
                .await
                .expect("noop device creation succeeds")
                .0
        })
    }

    fn image_desc() -> TextureDescriptor {
        TextureDescriptor::new([4, 4], wgpu::TextureFormat::Rgba8Unorm)
    }

    #[test]
    fn image_reuse_requires_exact_descriptor_and_exclusive_checkout() {
        let device = device();
        let mut pool = ScratchPool::new();
        let first = pool.take_image(&device, image_desc());
        let first_handle = first.texture.clone();
        assert!(pool.return_image(first));
        let reused = pool.take_image(&device, image_desc());
        assert_eq!(reused.texture, first_handle);
        let simultaneous = pool.take_image(&device, image_desc());
        assert_ne!(reused.texture, simultaneous.texture);
        assert_eq!(pool.stats().checked_out_bytes, 128);
        assert_eq!(pool.stats().peak_bytes, 128);
        assert!(pool.return_image(reused));
        assert!(pool.return_image(simultaneous));
        let mut different = image_desc();
        different.usages = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let rendered = pool.take_image(&device, different);
        assert_ne!(rendered.texture, first_handle);
        assert_eq!(pool.stats().image_allocations, 3);
        assert_eq!(pool.stats().image_reuses, 1);
        assert!(pool.return_image(rendered));
        pool.begin_frame().expect("all outputs returned");
        assert_eq!(pool.stats().image_allocations, 0);
        assert_eq!(pool.stats().pooled_bytes, 192);
        assert_eq!(pool.stats().peak_bytes, 192);
    }

    #[test]
    fn matching_vertex_index_outputs_remain_distinct_and_duplicate_returns_are_rejected() {
        let device = device();
        let mut pool = ScratchPool::new();
        let usage = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX;
        let first = pool.take_buffer(&device, 64, usage);
        let first_handle = first.clone();
        assert!(pool.return_buffer(first));
        assert!(!pool.return_buffer(first_handle.clone()));
        let vertices = pool.take_buffer(&device, 64, usage);
        let indices = pool.take_buffer(&device, 64, usage);
        assert_eq!(vertices, first_handle);
        assert_ne!(vertices, indices);
        assert_eq!(pool.stats().buffer_reuses, 1);
        assert_eq!(pool.stats().buffer_allocations, 2);
        assert_eq!(pool.stats().checked_out_bytes, 128);
        assert!(pool.return_buffer(vertices));
        assert!(pool.return_buffer(indices));
        let storage = pool.take_buffer(&device, 64, wgpu::BufferUsages::STORAGE);
        assert_ne!(storage, first_handle);
        assert_eq!(pool.stats().buffer_allocations, 3);
        assert!(pool.return_buffer(storage));
    }

    #[test]
    fn bounded_pool_prefers_recent_returns_and_never_retains_oversized_outputs() {
        let device = device();
        let mut pool = ScratchPool::with_budget(96);
        let old = pool.take_buffer(&device, 64, wgpu::BufferUsages::VERTEX);
        let old_handle = old.clone();
        assert!(pool.return_buffer(old));
        let image = pool.take_image(&device, image_desc());
        assert_eq!(pool.stats().peak_bytes, 128);
        assert!(pool.return_image(image));
        assert_eq!(pool.stats().pooled_bytes, 64);
        assert_eq!(pool.pooled.len(), 1);
        let replacement = pool.take_buffer(&device, 64, wgpu::BufferUsages::VERTEX);
        assert_ne!(replacement, old_handle);
        assert!(pool.return_buffer(replacement));
        let too_large = pool.take_buffer(&device, 128, wgpu::BufferUsages::VERTEX);
        assert!(!pool.return_buffer(too_large));
        assert_eq!(pool.stats().pooled_bytes, 64);
        pool.set_budget(0);
        assert_eq!(pool.stats().pooled_bytes, 0);
        assert!(pool.pooled.is_empty());
        let transient = pool.take_buffer(&device, 16, wgpu::BufferUsages::VERTEX);
        assert!(!pool.return_buffer(transient));
        assert_eq!(pool.stats().checked_out_bytes, 0);
    }

    #[test]
    fn descriptor_churn_cannot_accumulate_historical_buckets() {
        let device = device();
        let mut pool = ScratchPool::with_budget(256);
        for size in 1..=128 {
            let output = pool.take_buffer(&device, size * 4, wgpu::BufferUsages::VERTEX);
            pool.return_buffer(output);
            assert!(pool.stats().pooled_bytes <= 256);
            assert_eq!(pool.stats().checked_out_bytes, 0);
            assert_eq!(
                pool.stats().pooled_bytes,
                pool.pooled.iter().map(Output::bytes).sum()
            );
        }
        assert!(pool.pooled.len() <= 64);
        pool.reset();
        assert!(pool.pooled.is_empty());
        assert_eq!(pool.stats().pooled_bytes, 0);
    }

    #[test]
    fn reset_reconciles_aborted_checkouts_and_rejects_late_returns() {
        let device = device();
        let mut pool = ScratchPool::new();
        let image = pool.take_image(&device, image_desc());
        let buffer = pool.take_buffer(&device, 32, wgpu::BufferUsages::VERTEX);
        assert!(pool.begin_frame().is_err());
        assert_eq!(pool.stats().checked_out_bytes, 96);
        pool.reset();
        assert_eq!(pool.stats().checked_out_bytes, 0);
        assert_eq!(pool.stats().peak_bytes, 96);
        assert!(!pool.return_image(image));
        assert!(!pool.return_buffer(buffer));
        pool.begin_frame().expect("abort bookkeeping reset");
        assert_eq!(pool.stats(), ScratchStats::default());
    }

    #[test]
    fn a_mutated_image_descriptor_cannot_poison_reuse() {
        let device = device();
        let mut pool = ScratchPool::new();
        let mut image = pool.take_image(&device, image_desc());
        image.desc.size = [2, 2];
        assert!(!pool.return_image(image));
        assert_eq!(pool.stats().checked_out_bytes, 0);
        assert_eq!(pool.stats().pooled_bytes, 0);
        pool.begin_frame()
            .expect("rejected output accounting is reconciled");
    }
}
