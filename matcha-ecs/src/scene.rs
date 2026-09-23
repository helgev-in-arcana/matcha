//! Composition of native rendering-interface Scenes, not another paint tree.
//!
//! Widget RenderItems cache complete local Scenes including their GPU generators.
//! Composition resolves local coordinates and mask indices, and imports shared
//! source definitions. No pixels are generated here. Widgets may submit arbitrary
//! meshes, GPU render/compute generators and multiple phases directly.

use render_interface::*;
use std::sync::LazyLock;

#[derive(Debug, thiserror::Error)]
pub enum CompositionError {
    #[error(transparent)]
    Resource(#[from] DuplicateResource),
    #[error("invalid local/inherited mask reference")]
    InvalidMask,
    #[error("composed mask index capacity exceeded")]
    Capacity,
}

/// Shared source; cloning shares only its immutable generator allocation.
pub fn unit_quad() -> MeshSource {
    static QUAD: LazyLock<MeshSource> = LazyLock::new(|| {
        let mut desc = MeshDescriptor::triangles(6, 0);
        desc.bounds = Some([[0., 0., 0.], [1., 1., 0.]]);
        desc.non_overlapping = true;
        MeshSource::new(desc, |mut c| {
            let vertices = [
                Vertex {
                    position: [0., 0., 0.],
                    uv: [0., 0.],
                },
                Vertex {
                    position: [0., 1., 0.],
                    uv: [0., 1.],
                },
                Vertex {
                    position: [1., 1., 0.],
                    uv: [1., 1.],
                },
                Vertex {
                    position: [0., 0., 0.],
                    uv: [0., 0.],
                },
                Vertex {
                    position: [1., 1., 0.],
                    uv: [1., 1.],
                },
                Vertex {
                    position: [1., 0., 0.],
                    uv: [1., 0.],
                },
            ];
            upload_buffer(
                &mut c.gpu,
                c.target.vertices,
                bytemuck::cast_slice(&vertices),
            )
        })
    });
    QUAD.clone()
}

/// Apply placement to flat records. Phases remain absolute; only root masks
/// inherit parent. Source conflicts fail before the destination is changed.
///
/// Appending A then B produces A0/B0, then A1/B1, not all of A followed by B.
/// The resulting phase snapshot is global; sources are shared without rewriting
/// captured coordinates. Backdrop producers must use resolved RenderCtx placement.
/// Opacity multiplies each object, not an isolated group's composited result.
pub fn append_scene(
    destination: &mut Scene,
    source: &Scene,
    transform: Matrix4<f32>,
    parent: Option<PixelMaskIndex>,
    opacity: f32,
) -> Result<(), CompositionError> {
    if parent.is_some_and(|i| i.0 as usize >= destination.pixel_masks.len())
        || source
            .pixel_masks
            .iter()
            .enumerate()
            .any(|(n, m)| m.parent.is_some_and(|i| i.0 as usize >= n))
        || source.phases.iter().flat_map(|p| &p.objects).any(|o| {
            o.mask
                .is_some_and(|i| i.0 as usize >= source.pixel_masks.len())
        })
    {
        return Err(CompositionError::InvalidMask);
    }
    if destination
        .pixel_masks
        .len()
        .checked_add(source.pixel_masks.len())
        .is_none_or(|n| n > u32::MAX as usize)
    {
        return Err(CompositionError::Capacity);
    }
    destination.resources.import(&source.resources)?;
    let offset = destination.pixel_masks.len() as u32;
    destination
        .pixel_masks
        .extend(source.pixel_masks.iter().map(|m| PixelMask {
            mesh: m.mesh,
            texture: m.texture,
            transform: transform * m.transform,
            parent: m.parent.map(|i| PixelMaskIndex(i.0 + offset)).or(parent),
        }));
    destination.phases.resize_with(
        destination.phases.len().max(source.phases.len()),
        Phase::default,
    );
    for (target, phase) in destination.phases.iter_mut().zip(&source.phases) {
        target.objects.extend(phase.objects.iter().map(|o| Object {
            mesh: o.mesh,
            texture: o.texture,
            transform: transform * o.transform,
            mask: o.mask.map(|i| PixelMaskIndex(i.0 + offset)).or(parent),
            opacity: opacity * o.opacity,
        }));
    }
    Ok(())
}

/// A convenience for standard textured rectangles. Returns the native contract
/// directly; callers may freely add meshes, phases and generators to it.
pub fn textured_quad(
    texture: TextureSource,
    size: [f32; 2],
    position: Matrix4<f32>,
    mask: Option<MaskSource>,
) -> Scene {
    let mut scene = Scene::default();
    push_quad(&mut scene, &texture, size, position, mask.as_ref());
    scene
}
/// Append directly to flat storage; glyph runs use this without one Scene/Map
/// allocation per glyph. Registration shares an existing source definition.
pub fn push_quad(
    scene: &mut Scene,
    texture: &TextureSource,
    size: [f32; 2],
    position: Matrix4<f32>,
    mask: Option<&MaskSource>,
) {
    let mesh = scene
        .resources
        .share_mesh(&unit_quad())
        .expect("shared quad definition");
    let texture = scene
        .resources
        .share_texture(texture)
        .expect("shared texture definition");
    let transform =
        position * Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(size[0], size[1], 1.));
    let mut object = Object::new(mesh, texture, transform);
    if let Some(mask) = mask {
        let texture = scene
            .resources
            .share_mask(mask)
            .expect("shared mask definition");
        let index = PixelMaskIndex(scene.pixel_masks.len() as u32);
        scene.pixel_masks.push(PixelMask {
            mesh,
            texture,
            transform,
            parent: None,
        });
        object.mask = Some(index);
    }
    if scene.phases.is_empty() {
        scene.phases.push(Phase::default());
    }
    scene.phases[0].objects.push(object);
}

/// Append a local Scene when its source definitions are constructed by the UI.
pub fn append_local(destination: &mut Scene, source: Scene, transform: Matrix4<f32>) {
    append_scene(destination, &source, transform, None, 1.)
        .expect("UI shares cloned source definitions for repeated IDs");
}
