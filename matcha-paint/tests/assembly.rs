use matcha_paint::*;
use render_interface::*;

#[test]
fn persistent_pool_and_self_only_mask_survive_reassembly() {
    let color = Bitmap::rgba([1, 1], vec![255; 4]).expect("color");
    let coverage = Bitmap::coverage([1, 1], vec![128]).expect("coverage");
    let child = RenderNode::new().with_texture(color.clone(), [2., 2.], Matrix4::identity());
    let root = RenderNode::new()
        .with_texture(color.clone(), [4., 4.], Matrix4::identity())
        .with_stencil(coverage, [4., 4.], Matrix4::identity())
        .add_child(child, Matrix4::identity());
    let mut builder = SceneBuilder::new();
    let mut source_address = std::ptr::null();
    for frame in 0..2 {
        builder.begin();
        let clip = builder.push_clip(None, Matrix4::identity());
        builder.push(&root, Matrix4::identity(), Some(clip), 0.5);
        builder.finish();
        let scene = builder.scene();
        let objects = &scene.phases[0].objects;
        assert_eq!(objects.len(), 2);
        assert_ne!(objects[0].mask, Some(clip));
        assert_eq!(objects[1].mask, Some(clip));
        assert_eq!(objects[1].opacity, 0.5);
        assert_eq!(scene.resources.len(), 4);
        let address = scene
            .resources
            .texture(color.texture_id())
            .expect("definition") as *const TextureSource;
        if frame == 0 {
            source_address = address;
        } else {
            assert_eq!(
                source_address, address,
                "no source reconstruction/reallocation"
            );
        }
    }
}

#[test]
fn custom_contributor_receives_resolved_placement_and_can_escape_clip() {
    let bitmap = Bitmap::rgba([1, 1], vec![255; 4]).expect("color");
    let node = RenderNode::custom(move |scene, placement| {
        let q = scene
            .resources
            .insert_mesh(unit_quad())
            .expect("custom quad");
        let t = bitmap.register_texture(&mut scene.resources);
        scene
            .phases
            .resize_with(placement.phase + 1, Phase::default);
        scene.phases[placement.phase]
            .objects
            .push(Object::new(q, t, placement.transform));
    })
    .in_phase(2);
    let mut builder = SceneBuilder::new();
    builder.begin();
    let clip = builder.push_clip(None, Matrix4::identity());
    builder.push(&node, Matrix4::identity(), Some(clip), 1.);
    builder.finish();
    assert!(builder.scene().phases[2].objects[0].mask.is_none());
}
