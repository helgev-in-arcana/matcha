//! Records one frame without submitting it. The facade submits only after all
//! CPU callbacks succeed; resource publication follows the same boundary.
use crate::{RenderStats, SceneError, SceneTarget, compositor::*, plan::FramePlan, resources::*};
use render_interface::*;
use std::collections::HashMap;
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
pub(crate) fn encode(
    device: &wgpu::Device,
    compositor: &mut Compositor,
    resources: &mut ResourceStore,
    scene: &Scene,
    plan: &FramePlan,
    target: SceneTarget<'_>,
    s: &Surfaces,
    stats: &mut RenderStats,
) -> Result<DrawFrame, SceneError> {
    let stride = u64::from(device.limits().min_uniform_buffer_offset_alignment)
        .max(112)
        .next_multiple_of(u64::from(
            device.limits().min_uniform_buffer_offset_alignment,
        ));
    let mut draw_capacity = 2usize;
    for object in scene.phases.iter().flat_map(|p| &p.objects) {
        draw_capacity += 1;
        let mut mask = object.mask;
        while let Some(i) = mask {
            draw_capacity += 2;
            mask = scene.pixel_masks[i.0 as usize].parent;
        }
    }
    let uniform_size = stride
        .checked_mul(draw_capacity as u64)
        .ok_or_else(|| SceneError::Invalid("draw parameter capacity overflow".into()))?;
    if uniform_size > device.limits().max_buffer_size || uniform_size > u64::from(u32::MAX) {
        return Err(SceneError::Invalid("too many draw parameters".into()));
    }
    if compositor
        .parameter_buffer
        .as_ref()
        .is_none_or(|buffer| buffer.size() < uniform_size)
    {
        compositor.parameter_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("reusable frame parameter arena"),
            size: uniform_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }
    let mut parameter_bytes = std::mem::take(&mut compositor.parameter_bytes);
    parameter_bytes.clear();
    let mut frame = DrawFrame {
        encoder: device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("scene frame"),
        }),
        uniforms: compositor
            .parameter_buffer
            .as_ref()
            .expect("arena was allocated")
            .clone(),
        bytes: parameter_bytes,
        stride: stride as usize,
        pending: Vec::new(),
        destination: None,
        batches: 0,
        groups: HashMap::new(),
    };
    clear(&mut frame.encoder, &s.color.view, target.clear);
    let full = Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(
        target.viewport[0],
        target.viewport[1],
        1.,
    ));
    if let Some(initial) = target.initial {
        compositor.draw(
            &mut frame,
            &s.color.view,
            &compositor.color_pipeline,
            &compositor.quad,
            initial.into(),
            &compositor.white.view,
            Params {
                transform: full,
                viewport: target.viewport,
                opacity: 1.,
                masked: 0,
            },
            None,
            None,
        );
    }
    let mut chain = Vec::new();
    let mut last_mask = None;
    let mut active_mask = 0;
    let mut prefix = Vec::new();
    for (phase, preparation) in scene.phases.iter().zip(&plan.phases) {
        flush(&mut frame);
        // Every generator records its reads before any draw in this phase.
        // Therefore queue/encoder order freezes the accumulated image for
        // those reads; no full-viewport snapshot copy is necessary. Source
        // outputs are isolated and may not alias or mutate this input.
        let snapshot = RenderSnapshot {
            color_texture: &s.color.texture,
            color_view: &s.color.view,
            size: s.size,
            format: COLOR,
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
        for object in &phase.objects {
            let direct = object.mask.and_then(|i| {
                let m = &scene.pixel_masks[i.0 as usize];
                (m.mesh == object.mesh
                    && m.transform == object.transform
                    && resources.meshes[&m.mesh].value.desc.non_overlapping)
                    .then_some(m)
            });
            let screen_mask = direct.map_or(object.mask, |m| m.parent);
            let local_mask = direct.map(|m| ImageRef::from(&resources.masks[&m.texture].value));
            let mut object_bounds = pixel_bounds(
                resources.meshes[&object.mesh].value.desc.bounds,
                &object.transform,
                target.viewport,
                s.size,
            );
            chain.clear();
            let mut mask = screen_mask;
            while let Some(index) = mask {
                chain.push(index);
                mask = scene.pixel_masks[index.0 as usize].parent;
            }
            chain.reverse();
            for index in &chain {
                let node = &scene.pixel_masks[index.0 as usize];
                object_bounds = intersection(
                    object_bounds,
                    pixel_bounds(
                        resources.meshes[&node.mesh].value.desc.bounds,
                        &node.transform,
                        target.viewport,
                        s.size,
                    ),
                );
            }
            if object_bounds[2] == 0 || object_bounds[3] == 0 {
                continue;
            }
            if screen_mask.is_some() && screen_mask != last_mask {
                let common = prefix
                    .iter()
                    .zip(&chain)
                    .take_while(|(a, b)| a == b)
                    .count();
                let mut bounds = [0, 0, s.size[0], s.size[1]];
                for (depth, index) in chain.iter().enumerate() {
                    let node = &scene.pixel_masks[index.0 as usize];
                    bounds = intersection(
                        bounds,
                        pixel_bounds(
                            resources.meshes[&node.mesh].value.desc.bounds,
                            &node.transform,
                            target.viewport,
                            s.size,
                        ),
                    );
                    active_mask = mask_slot(depth);
                    if depth < common {
                        continue;
                    }
                    // Clear only the conservative intersection. Pixels outside
                    // it are never sampled by a surviving object's scissor.
                    compositor.draw(
                        &mut frame,
                        &s.masks[active_mask].view,
                        &compositor.clear_pipeline,
                        &compositor.quad,
                        (&compositor.white).into(),
                        &compositor.white.view,
                        Params {
                            transform: full,
                            viewport: target.viewport,
                            opacity: 0.,
                            masked: 0,
                        },
                        Some(bounds),
                        None,
                    );
                    let parent = if depth == 0 {
                        &compositor.white.view
                    } else {
                        &s.masks[mask_slot(depth - 1)].view
                    };
                    compositor.draw(
                        &mut frame,
                        &s.masks[active_mask].view,
                        &compositor.mask_pipeline,
                        &resources.meshes[&node.mesh].value,
                        (&resources.masks[&node.texture].value).into(),
                        parent,
                        Params {
                            transform: node.transform,
                            viewport: target.viewport,
                            opacity: 1.,
                            masked: u32::from(depth != 0),
                        },
                        Some(bounds),
                        None,
                    );
                    stats.mask_passes += 1;
                }
                prefix.clear();
                prefix.extend(chain.iter().take(4).copied());
            }
            last_mask = screen_mask;
            compositor.draw(
                &mut frame,
                &s.color.view,
                &compositor.color_pipeline,
                &resources.meshes[&object.mesh].value,
                (&resources.textures[&object.texture].value).into(),
                &s.masks[active_mask].view,
                Params {
                    transform: object.transform,
                    viewport: target.viewport,
                    opacity: object.opacity,
                    masked: u32::from(screen_mask.is_some())
                        | if local_mask.is_some() { 2 } else { 0 },
                },
                Some(object_bounds),
                local_mask,
            );
            stats.draw_calls += 1;
        }
    }
    let format = target.format;
    let output = compositor
        .outputs
        .entry(format)
        .or_insert_with(|| {
            pipeline(
                &device,
                &compositor.pipeline_layout,
                &compositor.shader,
                format,
                "color",
                None,
            )
        })
        .clone();
    // No Load of uninitialized destination content: output covers the whole
    // attachment and replaces it; clear also makes empty scenes defined.
    flush(&mut frame);
    clear(&mut frame.encoder, target.view, wgpu::Color::TRANSPARENT);
    compositor.draw(
        &mut frame,
        target.view,
        &output,
        &compositor.quad,
        (&s.color).into(),
        &compositor.white.view,
        Params {
            transform: full,
            viewport: target.viewport,
            opacity: 1.,
            masked: 0,
        },
        None,
        None,
    );
    flush(&mut frame);
    stats.draw_batches = frame.batches;
    stats.bind_groups = frame.groups.len();
    Ok(frame)
}
