//! CPU-only scheduling proofs. GPU generators must never execute in assembly.
use matcha_ecs::scene::{Frame, unit_quad};
use render_interface::*;

fn texture() -> TextureSource {
    TextureSource::new(
        TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
        |_| panic!("assembly is CPU-only"),
    )
}

#[test]
fn backdrop_boundaries_follow_paint_order_across_widget_writers() {
    let mut frame = Frame::default();
    let textures = [texture(), texture(), texture(), texture(), texture()];
    frame.begin();
    {
        let mut draw = frame.draw(Matrix4::identity(), None, 1.);
        let mesh = draw.mesh(&unit_quad());
        let a = draw.texture(&textures[0]);
        let effect = draw.texture(&textures[1]);
        draw.object(Object::new(mesh, a, Matrix4::identity()));
        draw.backdrop(Object::new(mesh, effect, Matrix4::identity()));
    }
    // B's background must include A's effect, not only A's ordinary content.
    {
        let mut draw = frame.draw(Matrix4::identity(), None, 1.);
        let mesh = draw.mesh(&unit_quad());
        let b = draw.texture(&textures[2]);
        let effect = draw.texture(&textures[3]);
        draw.object(Object::new(mesh, b, Matrix4::identity()));
        draw.backdrop(Object::new(mesh, effect, Matrix4::identity()));
        let top = draw.texture(&textures[4]);
        draw.object(Object::new(mesh, top, Matrix4::identity()));
    }
    frame.finish().expect("valid frame");
    let order: Vec<Vec<_>> = frame
        .scene
        .phases
        .iter()
        .map(|p| p.objects.iter().map(|o| o.texture).collect())
        .collect();
    assert_eq!(
        order,
        vec![
            vec![textures[0].id()],
            vec![textures[1].id(), textures[2].id()],
            vec![textures[3].id(), textures[4].id()]
        ]
    );
}

#[test]
fn draw_records_rebuild_but_definitions_and_mask_placement_are_reused() {
    let mut frame = Frame::default();
    let t = texture();
    let hint = texture();
    let mask = MaskSource::new(
        MaskDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
        |_| panic!("not GPU"),
    );
    let placed = Matrix4::new_translation(&nalgebra::Vector3::new(10., 20., 0.));
    for _ in 0..3 {
        frame.begin();
        let mut draw = frame.draw(placed, None, 0.5);
        draw.texture(&hint);
        draw.quad(&t, [1., 1.], Matrix4::identity(), Some(&mask));
        draw.translated(placed, |draw| {
            draw.quad(&t, [1., 1.], Matrix4::identity(), None)
        });
        frame.finish().expect("valid");
        assert_eq!(frame.scene.resources.len(), 4);
        assert_eq!(frame.scene.phases[0].objects.len(), 2);
        assert_eq!(frame.scene.phases[0].objects[0].opacity, 0.5);
        assert_eq!(frame.scene.phases[0].objects[1].transform, placed * placed);
        assert_eq!(frame.scene.pixel_masks[0].transform, placed);
    }
    frame.begin();
    frame.finish().expect("empty");
    assert!(frame.scene.resources.is_empty());
    assert!(frame.scene.phases[0].objects.is_empty());
}

#[test]
fn failed_frames_prune_definitions_and_recover() {
    let mut frame = Frame::default();
    for _ in 0..20 {
        frame.begin();
        let t = texture();
        let conflict = TextureSource::with_id(
            t.id(),
            TextureDescriptor::new([2, 2], wgpu::TextureFormat::Rgba8Unorm),
            |_| Ok(()),
        );
        let mut draw = frame.draw(Matrix4::identity(), None, 1.);
        draw.texture(&t);
        draw.texture(&conflict);
        assert!(frame.finish().is_err());
        assert_eq!(frame.scene.resources.len(), 1);
    }
    frame.begin();
    let mut draw = frame.draw(Matrix4::identity(), None, 1.);
    draw.object(Object {
        mask: Some(PixelMaskIndex(u32::MAX)),
        ..Object::new(MeshId::new(), TextureId::new(), Matrix4::identity())
    });
    assert!(frame.finish().is_err());
    frame.begin();
    frame.finish().expect("repair");
    assert!(frame.scene.resources.is_empty());
}

#[test]
fn mask_scopes_inherit_coverage_and_restore_sibling_state() {
    let mut frame = Frame::default();
    frame.begin();
    let texture = texture();
    let mesh = unit_quad();
    let mask = MaskSource::new(
        MaskDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
        |_| panic!("CPU only"),
    );
    let translated = Matrix4::new_translation(&nalgebra::Vector3::new(3., 5., 0.));
    let mut draw = frame.draw(Matrix4::identity(), None, 1.);
    draw.masked(&mesh, &mask, translated, |draw| {
        draw.translated(translated, |draw| {
            draw.masked(&mesh, &mask, Matrix4::identity(), |draw| {
                draw.quad(&texture, [1., 1.], Matrix4::identity(), None);
            })
        });
        draw.quad(&texture, [1., 1.], Matrix4::identity(), None);
    });
    draw.quad(&texture, [1., 1.], Matrix4::identity(), None);
    frame.finish().expect("scoped masks");
    assert_eq!(frame.scene.pixel_masks[1].parent, Some(PixelMaskIndex(0)));
    assert_eq!(frame.scene.pixel_masks[0].transform, translated);
    assert_eq!(
        frame.scene.pixel_masks[1].transform, translated,
        "mask geometry does not inherit parent transform"
    );
    let objects = &frame.scene.phases[0].objects;
    assert_eq!(
        objects.iter().map(|o| o.mask).collect::<Vec<_>>(),
        vec![Some(PixelMaskIndex(1)), Some(PixelMaskIndex(0)), None]
    );
    assert_eq!(objects[0].transform, translated);
    assert_eq!(objects[1].transform, Matrix4::identity());
}
