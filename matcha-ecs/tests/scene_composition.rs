//! Native Scene composition is CPU-only; no widget generator is executed here.
use matcha_ecs::scene::{CompositionError, append_scene, unit_quad};
use render_interface::*;
fn texture() -> TextureSource {
    TextureSource::new(
        TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
        |_| panic!("composition may not generate"),
    )
}
fn mask() -> MaskSource {
    MaskSource::new(
        MaskDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
        |_| panic!("composition may not generate"),
    )
}
#[test]
fn multiple_local_scenes_share_sources_rebase_masks_and_keep_phase_order() {
    let q = unit_quad();
    let t = texture();
    let m = mask();
    let candidate = texture();
    let mut widget = Scene::default();
    widget.resources.share_mesh(&q).expect("mesh");
    widget.resources.share_texture(&t).expect("texture");
    widget.resources.share_mask(&m).expect("mask");
    widget
        .resources
        .share_texture(&candidate)
        .expect("retention hint");
    widget.pixel_masks.push(PixelMask {
        mesh: q.id(),
        texture: m.id(),
        transform: Matrix4::identity(),
        parent: None,
    });
    widget.phases = vec![
        Phase::default(),
        Phase {
            objects: vec![Object {
                mask: Some(PixelMaskIndex(0)),
                ..Object::new(q.id(), t.id(), Matrix4::identity())
            }],
        },
    ];
    let mut frame = Scene::default();
    frame
        .resources
        .import(&widget.resources)
        .expect("definitions");
    frame.pixel_masks.push(widget.pixel_masks[0].clone());
    let placed = Matrix4::new_translation(&nalgebra::Vector3::new(10., 20., 0.));
    append_scene(&mut frame, &widget, placed, Some(PixelMaskIndex(0)), 0.5)
        .expect("first instance");
    append_scene(&mut frame, &widget, Matrix4::identity(), None, 1.)
        .expect("unclipped popup instance");
    assert_eq!(
        frame.resources.len(),
        4,
        "definitions shared once, including unused hint"
    );
    assert_eq!(frame.pixel_masks[1].parent, Some(PixelMaskIndex(0)));
    assert_eq!(frame.pixel_masks[2].parent, None);
    assert_eq!(frame.phases[1].objects[0].mask, Some(PixelMaskIndex(1)));
    assert_eq!(frame.phases[1].objects[1].mask, Some(PixelMaskIndex(2)));
    assert_eq!(frame.phases[1].objects[0].transform, placed);
    assert_eq!(frame.phases[1].objects[0].opacity, 0.5);
}
#[test]
fn malformed_indices_and_conflicting_definitions_fail_before_mutation() {
    let mut frame = Scene::default();
    let t = texture();
    frame.resources.share_texture(&t).expect("original");
    let mut bad = Scene::default();
    bad.resources
        .insert_texture(TextureSource::with_id(
            t.id(),
            TextureDescriptor::new([2, 2], wgpu::TextureFormat::Rgba8Unorm),
            |_| Ok(()),
        ))
        .expect("conflicting descriptor");
    assert!(matches!(
        append_scene(&mut frame, &bad, Matrix4::identity(), None, 1.),
        Err(CompositionError::Resource(_))
    ));
    assert_eq!(frame.resources.len(), 1);
    bad = Scene::default();
    bad.phases.push(Phase {
        objects: vec![Object {
            mask: Some(PixelMaskIndex(u32::MAX)),
            ..Object::new(MeshId::new(), TextureId::new(), Matrix4::identity())
        }],
    });
    assert!(matches!(
        append_scene(&mut frame, &bad, Matrix4::identity(), None, 1.),
        Err(CompositionError::InvalidMask)
    ));
    assert!(frame.phases.is_empty());
    assert!(frame.pixel_masks.is_empty());
    assert_eq!(frame.resources.len(), 1);
}
