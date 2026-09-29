//! Records one frame without submitting it. The facade submits only after all
//! CPU callbacks succeed; resource publication follows the same boundary.
use crate::compositor::plan::{DrawOp, DrawPlan};
use crate::{PlainError, PlainTarget, RenderStats, compositor::*, plan::FramePlan, resources::*};
use render_interface::*;

/// An unwind is transported to the submission owner only after the encoder is
/// discarded and compositor workspace is recovered. It is resumed there, never
/// changed into a successful render or a provider's ordinary PrepareError.
pub(crate) enum FrameFailure {
    Recording(PlainError),
    Unwind(Box<dyn std::any::Any + Send>),
}
impl From<PlainError> for FrameFailure {
    fn from(error: PlainError) -> Self {
        Self::Recording(error)
    }
}
pub(crate) struct Surfaces {
    pub(crate) size: [u32; 2],
    pub(crate) color: Image,
    pub(crate) masks: [Image; 6],
}
impl Surfaces {
    pub(crate) fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        Self {
            size,
            color: attachment(device, size, COLOR),
            masks: std::array::from_fn(|_| attachment(device, size, COVERAGE)),
        }
    }
}
pub(crate) fn attachment(
    device: &wgpu::Device,
    size: [u32; 2],
    format: wgpu::TextureFormat,
) -> Image {
    make_image(
        device,
        TextureDescriptor {
            size,
            format,
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        },
    )
}
#[allow(clippy::too_many_arguments)] // Explicitly borrowed, separately owned subsystems.
pub(crate) fn encode(
    device: &wgpu::Device,
    compositor: &mut Compositor,
    resources: &mut ResourceStore,
    scene: &Scene,
    plan: &FramePlan,
    target: PlainTarget<'_>,
    s: &Surfaces,
    stats: &mut RenderStats,
) -> Result<DrawFrame, FrameFailure> {
    // Taking only the reusable CPU plan avoids borrowing the compositor through
    // its own draw calls. Both success and every ordinary error restore it.
    let mut draw_plan = std::mem::take(&mut compositor.draw_plan);
    let result = match draw_plan.rebuild(scene, target.logical_size, s.size) {
        Ok(()) => encode_planned(
            device, compositor, resources, scene, plan, &draw_plan, target, s, stats,
        ),
        Err(error) => Err(error.into()),
    };
    compositor.draw_plan = draw_plan;
    if result.is_err() {
        compositor.clear_bind_groups();
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn encode_planned(
    device: &wgpu::Device,
    compositor: &mut Compositor,
    resources: &mut ResourceStore,
    scene: &Scene,
    plan: &FramePlan,
    draw_plan: &DrawPlan,
    target: PlainTarget<'_>,
    s: &Surfaces,
    stats: &mut RenderStats,
) -> Result<DrawFrame, FrameFailure> {
    let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment);
    let stride = PARAMETER_BYTES
        .checked_add(alignment.saturating_sub(1))
        .and_then(|size| size.checked_div(alignment))
        .and_then(|units| units.checked_mul(alignment))
        .ok_or_else(|| PlainError::Invalid("draw parameter alignment overflow".into()))?;
    let uniform_count = draw_plan
        .uniform_count
        .checked_add(usize::from(target.initial.is_some()))
        .and_then(|count| count.checked_add(1))
        .and_then(|count| u64::try_from(count).ok())
        .ok_or_else(|| PlainError::Invalid("draw operation count overflow".into()))?;
    let uniform_size = stride
        .checked_mul(uniform_count)
        .ok_or_else(|| PlainError::Invalid("draw parameter capacity overflow".into()))?;
    if uniform_size > device.limits().max_buffer_size || uniform_size > u64::from(u32::MAX) {
        return Err(PlainError::Invalid("too many draw parameters".into()).into());
    }
    if compositor
        .parameter_buffer
        .as_ref()
        .is_none_or(|buffer| buffer.size() < uniform_size)
    {
        // Bind groups include the uniform arena even though their view key
        // does not. Replacing that arena invalidates every cached binding.
        compositor.clear_bind_groups();
        compositor.parameter_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("reusable frame parameter arena"),
            size: uniform_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }
    let mut frame = DrawFrame {
        encoder: device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("plain renderer frame"),
        }),
        uniforms: compositor
            .parameter_buffer
            .as_ref()
            .expect("arena was allocated")
            .clone(),
        storage: compositor.take_workspace(),
        stride: stride as usize,
        destination: None,
        batches: 0,
    };
    // record_operations invokes source callbacks. AssertUnwindSafe is justified
    // by discarding the encoder, rolling back provisional residents, and restoring
    // the taken workspace before the panic leaves the renderer.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        record_operations(
            device, compositor, resources, scene, plan, draw_plan, target, s, stats, &mut frame,
        )
    }));
    let result = match result {
        Ok(result) => result.map_err(FrameFailure::Recording),
        Err(payload) => Err(FrameFailure::Unwind(payload)),
    };
    // These counters describe recording work, including work later discarded on
    // a CPU preparation error. They are not a receipt of GPU execution.
    stats.draw_batches = frame.batches;
    stats.bind_groups = frame.storage.bind_groups_created;
    if let Err(error) = result {
        // Discard recorded GPU work before making its CPU workspace available
        // again. Source-generated resident leases are rolled back by the facade.
        drop(frame.encoder);
        compositor.abort_workspace(frame.storage);
        return Err(error);
    }
    if frame.storage.bytes.len() as u64 != uniform_size {
        drop(frame.encoder);
        compositor.abort_workspace(frame.storage);
        return Err(
            PlainError::Invalid("draw plan and recorded uniform count differ".into()).into(),
        );
    }
    Ok(frame)
}

#[allow(clippy::too_many_arguments)]
fn record_operations(
    device: &wgpu::Device,
    compositor: &mut Compositor,
    resources: &mut ResourceStore,
    scene: &Scene,
    plan: &FramePlan,
    draw_plan: &DrawPlan,
    target: PlainTarget<'_>,
    s: &Surfaces,
    stats: &mut RenderStats,
    frame: &mut DrawFrame,
) -> Result<(), PlainError> {
    let color_region = s.color.target().region;
    let mask_regions = s.masks.each_ref().map(|mask| mask.target().region);
    clear(&mut frame.encoder, &s.color.view, target.clear);
    let full = Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(
        target.logical_size[0],
        target.logical_size[1],
        1.,
    ));
    if let Some(initial) = target.initial {
        compositor.draw(
            frame,
            color_region,
            &compositor.color_pipeline,
            &compositor.quad,
            initial.into(),
            &compositor.white.view,
            Params {
                transform: full,
                viewport: target.logical_size,
                opacity: 1.,
                masked: 0,
            },
            None,
            None,
        );
    }
    for ((phase, preparation), operations) in
        scene.phases.iter().zip(&plan.phases).zip(&draw_plan.phases)
    {
        flush(frame);
        // Every generator records its reads before any draw in this phase.
        // Therefore queue/encoder order freezes the accumulated image for
        // those reads; no full-image snapshot copy is necessary. Source
        // output textures are separate from this input and cannot mutate it.
        let snapshot = RenderSnapshot {
            color: color_region,
        };
        // Prepare all resources before drawing any object of this phase.
        for id in &preparation.meshes {
            resources.prepare_mesh(scene, *id, &mut frame.encoder, snapshot)?;
        }
        for id in &preparation.textures {
            resources.prepare_texture(scene, *id, &mut frame.encoder, snapshot)?;
        }
        for id in &preparation.masks {
            resources.prepare_mask(scene, *id, &mut frame.encoder, snapshot)?;
        }
        for operation in operations {
            match *operation {
                DrawOp::ClearMask { slot, scissor } => {
                    compositor.draw(
                        frame,
                        mask_regions[slot],
                        &compositor.clear_pipeline,
                        &compositor.quad,
                        (&compositor.white).into(),
                        &compositor.white.view,
                        Params {
                            transform: full,
                            viewport: target.logical_size,
                            opacity: 0.,
                            masked: 0,
                        },
                        Some(scissor),
                        None,
                    );
                }
                DrawOp::Mask {
                    index,
                    slot,
                    parent,
                    scissor,
                } => {
                    let node = &scene.pixel_masks[index];
                    let parent_view =
                        parent.map_or(&compositor.white.view, |slot| &s.masks[slot].view);
                    compositor.draw(
                        frame,
                        mask_regions[slot],
                        &compositor.mask_pipeline,
                        &resources.meshes[&node.mesh].value,
                        (&resources.masks[&node.texture].value).into(),
                        parent_view,
                        Params {
                            transform: node.transform,
                            viewport: target.logical_size,
                            opacity: 1.,
                            masked: u32::from(parent.is_some()),
                        },
                        Some(scissor),
                        None,
                    );
                    stats.mask_passes += 1;
                }
                DrawOp::Object {
                    index,
                    screen_mask,
                    local_mask,
                    scissor,
                } => {
                    let object = &phase.objects[index];
                    let local_mask = local_mask.map(|index| {
                        ImageRef::from(&resources.masks[&scene.pixel_masks[index].texture].value)
                    });
                    let coverage =
                        screen_mask.map_or(&compositor.white.view, |slot| &s.masks[slot].view);
                    compositor.draw(
                        frame,
                        color_region,
                        &compositor.color_pipeline,
                        &resources.meshes[&object.mesh].value,
                        (&resources.textures[&object.texture].value).into(),
                        coverage,
                        Params {
                            transform: object.transform,
                            viewport: target.logical_size,
                            opacity: object.opacity,
                            masked: u32::from(screen_mask.is_some())
                                | if local_mask.is_some() { 2 } else { 0 },
                        },
                        Some(scissor),
                        local_mask,
                    );
                    stats.draw_calls += 1;
                }
            }
        }
    }
    let format = target.region.view_format();
    let output = compositor
        .outputs
        .entry(format)
        .or_insert_with(|| {
            pipeline(
                device,
                &compositor.pipeline_layout,
                &compositor.shader,
                format,
                "color",
                None,
            )
        })
        .clone();
    // The cleared accumulation image defines every pixel, even for an empty
    // scene. Replace exactly the destination region and Load the attachment to
    // preserve its other pixels; an attachment-wide clear would destroy them.
    flush(frame);
    compositor.draw(
        frame,
        target.region,
        &output,
        &compositor.quad,
        (&s.color).into(),
        &compositor.white.view,
        Params {
            transform: full,
            viewport: target.logical_size,
            opacity: 1.,
            masked: 0,
        },
        None,
        None,
    );
    flush(frame);
    stats.draw_batches = frame.batches;
    stats.bind_groups = frame.storage.bind_groups_created;
    Ok(())
}
