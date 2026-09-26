//! Exercise actual widget writers without a Device. Source definitions and
//! final Scene geometry must remain independent of renderer GPU residency.

use std::{io::Cursor, sync::Arc};

use bevy_ecs::{entity::Entity, world::World};
use matcha_ecs::{
    components::{
        render::{RenderCtx, RenderItem},
        view::ViewChildren,
    },
    scene::Frame,
    view::run_view,
};
use matcha_ecs_widgets::{ColorRect, Image, ObjectFit};
use nalgebra::{Matrix4, Vector3};

fn children(world: &World, root: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(root)
        .expect("root child list")
        .slots
        .iter()
        .map(|(_, entity)| *entity)
        .collect()
}

fn write(
    frame: &mut Frame,
    world: &World,
    entity: Entity,
    size: [f32; 2],
    origin: [f32; 2],
    opacity: f32,
) {
    let transform = Matrix4::new_translation(&Vector3::new(origin[0], origin[1], 0.));
    let ctx = RenderCtx {
        transform,
        viewport_size: [512., 512.],
        size,
        focused: false,
        focus_within: false,
        hovered: false,
        active: false,
    };
    let item = world.get::<RenderItem>(entity).expect("widget draw writer");
    (item.builder)(&ctx, &mut frame.draw(transform, None, opacity));
}

#[test]
fn rounded_widgets_share_sources_and_recolor_without_replacing_coverage() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |scope| {
        for _ in 0..2 {
            scope.leaf(ColorRect::new(64., 32.).radius(8.).color([1., 0., 0., 1.]));
        }
    });
    let entities = children(&world, root);
    let mut frame = Frame::default();
    frame.begin();
    write(&mut frame, &world, entities[0], [64., 32.], [10., 20.], 1.);
    write(
        &mut frame,
        &world,
        entities[1],
        [64., 32.],
        [100., 20.],
        0.5,
    );
    frame.finish().expect("valid widget frame");

    assert_eq!(frame.scene.phases.len(), 1);
    let objects = &frame.scene.phases[0].objects;
    assert_eq!(objects.len(), 2);
    assert_eq!(objects[0].mesh, objects[1].mesh);
    assert_eq!(objects[0].texture, objects[1].texture);
    assert_eq!(objects[0].transform[(0, 3)], 10.);
    assert_eq!(objects[1].transform[(0, 3)], 100.);
    assert_eq!(objects[1].opacity, 0.5);
    assert_eq!(
        frame.scene.resources.len(),
        3,
        "one mesh, one tint, one mask"
    );
    assert_eq!(frame.scene.pixel_masks.len(), 2);
    let coverage = frame.scene.pixel_masks[0].texture;
    assert_eq!(coverage, frame.scene.pixel_masks[1].texture);
    let red = objects[0].texture;

    run_view(&mut world, root, |scope| {
        scope.leaf(ColorRect::new(64., 32.).radius(8.).color([0., 1., 0., 1.]));
        scope.leaf(ColorRect::new(64., 32.).radius(8.).color([1., 0., 0., 1.]));
    });
    frame.begin();
    write(&mut frame, &world, entities[0], [64., 32.], [30., 40.], 1.);
    write(&mut frame, &world, entities[1], [64., 32.], [120., 40.], 1.);
    frame.finish().expect("recolored widget frame");
    assert_ne!(frame.scene.phases[0].objects[0].texture, red);
    assert_eq!(frame.scene.phases[0].objects[1].texture, red);
    assert!(
        frame
            .scene
            .pixel_masks
            .iter()
            .all(|mask| mask.texture == coverage)
    );
    assert_eq!(
        frame.scene.resources.len(),
        4,
        "geometry is reused across colors"
    );
    assert_eq!(frame.scene.phases[0].objects[0].transform[(0, 3)], 30.);

    frame.begin();
    frame.finish().expect("empty frame");
    assert!(
        frame.scene.resources.is_empty(),
        "provider caches do not retain frame definitions"
    );
}

#[test]
fn image_writers_resolve_each_fit_mode_and_reuse_decoded_sources() {
    let pixels = image::RgbaImage::from_pixel(4, 2, image::Rgba([128, 64, 32, 128]));
    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .expect("test PNG");
    let bytes: Arc<[u8]> = encoded.into_inner().into();
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, move |scope| {
        for fit in [
            ObjectFit::Contain,
            ObjectFit::Fill,
            ObjectFit::Cover,
            ObjectFit::ScaleDown,
        ] {
            scope.leaf(Image::from_bytes(bytes.clone(), 8., 8.).fit(fit));
        }
    });
    let entities = children(&world, root);
    let mut frame = Frame::default();
    let expected = [
        ([8, 4], [3., 9.]),
        ([8, 8], [3., 7.]),
        ([8, 8], [3., 7.]),
        ([4, 2], [5., 10.]),
    ];
    let mut previous = None;
    for _ in 0..2 {
        frame.begin();
        for &entity in &entities {
            write(&mut frame, &world, entity, [8., 8.], [3., 7.], 1.);
        }
        frame.finish().expect("image widget frame");
        let objects = &frame.scene.phases[0].objects;
        assert_eq!(objects.len(), expected.len());
        for (object, (size, origin)) in objects.iter().zip(expected) {
            let source = frame
                .scene
                .resources
                .texture(object.texture)
                .expect("image source");
            assert_eq!(source.descriptor().size, size);
            assert_eq!(object.transform[(0, 0)], size[0] as f32);
            assert_eq!(object.transform[(1, 1)], size[1] as f32);
            assert_eq!(object.transform[(0, 3)], origin[0]);
            assert_eq!(object.transform[(1, 3)], origin[1]);
            assert!(object.mask.is_none());
        }
        let ids: Vec<_> = objects.iter().map(|object| object.texture).collect();
        if let Some(previous) = previous {
            assert_eq!(
                ids, previous,
                "warm writers retain decoded image definitions"
            );
        }
        previous = Some(ids);
    }
}
