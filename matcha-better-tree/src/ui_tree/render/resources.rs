//! Submitted IDs belong to the framework's cached draw, not the Widget's source templates.
//! Future caller-managed resources can bypass remapping here while using the same Scene writer.
use std::collections::HashMap;

use render_interface::{
    MaskId, MaskSource, MeshId, MeshSource, ResourcePool, TextureId, TextureSource,
};

use crate::widget::{Draw, UiError};

// Reuse aliases inside one Draw (e.g. a color and mask sharing a mesh). Equal template IDs
// must describe equal content; ResourcePool checks descriptor/output-layout consistency.
pub(super) fn manage(mut draw: Draw) -> Result<Draw, UiError> {
    let mut templates = ResourcePool::default();
    let mut meshes = HashMap::<MeshId, MeshSource>::new();
    let mut textures = HashMap::<TextureId, TextureSource>::new();
    let mut masks = HashMap::<MaskId, MaskSource>::new();
    for color in &mut draw.color {
        templates.share_mesh(&color.mesh)?;
        templates.share_texture(&color.texture)?;
        color.mesh = meshes
            .entry(color.mesh.id())
            .or_insert_with(|| fresh_mesh(&color.mesh))
            .clone();
        color.texture = textures
            .entry(color.texture.id())
            .or_insert_with(|| fresh_texture(&color.texture))
            .clone();
    }
    if let Some(mask) = &mut draw.mask {
        templates.share_mesh(&mask.mesh)?;
        templates.share_mask(&mask.mask)?;
        mask.mesh = meshes
            .entry(mask.mesh.id())
            .or_insert_with(|| fresh_mesh(&mask.mesh))
            .clone();
        mask.mask = masks
            .entry(mask.mask.id())
            .or_insert_with(|| fresh_mask(&mask.mask))
            .clone();
    }
    Ok(draw)
}

fn fresh_mesh(source: &MeshSource) -> MeshSource {
    let source = source.clone();
    let layout = source.output_layout();
    MeshSource::new(*source.descriptor(), move |ctx| source.prepare(ctx)).with_output_layout(layout)
}

fn fresh_texture(source: &TextureSource) -> TextureSource {
    let source = source.clone();
    let layout = source.output_layout();
    TextureSource::new(*source.descriptor(), move |ctx| source.prepare(ctx))
        .with_output_layout(layout)
}

fn fresh_mask(source: &MaskSource) -> MaskSource {
    let source = source.clone();
    let layout = source.output_layout();
    MaskSource::new(*source.descriptor(), move |ctx| source.prepare(ctx)).with_output_layout(layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::{Color, Mask};
    use nalgebra::Matrix4;
    use render_interface::{MeshDescriptor, PrepareOutputLayout, TextureDescriptor};

    #[test]
    fn assigned_ids_are_fresh_but_aliases_and_output_layout_survive() {
        let mesh = MeshSource::new(MeshDescriptor::triangles(3, 0), |_| Ok(()))
            .with_output_layout(PrepareOutputLayout::AnyRegion);
        let texture = TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            |_| Ok(()),
        );
        let mask = MaskSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
            |_| Ok(()),
        );
        let draw = Draw {
            color: vec![Color {
                mesh: mesh.clone(),
                texture: texture.clone(),
                transform: Matrix4::identity(),
                opacity: 1.0,
            }]
            .into(),
            mask: Some(Mask {
                mesh: mesh.clone(),
                mask: mask.clone(),
                transform: Matrix4::identity(),
            }),
        };
        let first = manage(draw.clone()).unwrap();
        let second = manage(draw).unwrap();
        assert_ne!(first.color[0].mesh.id(), mesh.id());
        assert_ne!(first.color[0].texture.id(), texture.id());
        assert_ne!(first.mask.as_ref().unwrap().mask.id(), mask.id());
        assert_ne!(first.color[0].mesh.id(), second.color[0].mesh.id());
        assert_eq!(
            first.color[0].mesh.id(),
            first.mask.as_ref().unwrap().mesh.id()
        );
        assert_eq!(
            first.color[0].mesh.output_layout(),
            PrepareOutputLayout::AnyRegion
        );
    }

    #[test]
    fn conflicting_template_descriptors_are_reported_before_remapping() {
        let mesh = MeshSource::new(MeshDescriptor::triangles(3, 0), |_| Ok(()));
        let texture = TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            |_| Ok(()),
        );
        let conflicting =
            MeshSource::with_id(mesh.id(), MeshDescriptor::triangles(6, 0), |_| Ok(()));
        let draw = Draw {
            color: vec![
                Color {
                    mesh,
                    texture: texture.clone(),
                    transform: Matrix4::identity(),
                    opacity: 1.0,
                },
                Color {
                    mesh: conflicting,
                    texture,
                    transform: Matrix4::identity(),
                    opacity: 1.0,
                },
            ]
            .into(),
            mask: None,
        };
        assert!(matches!(manage(draw), Err(UiError::Resource(_))));
    }
}
