//! Descriptor and occurrence validation without creating a wgpu device.
use super::*;

fn repeated_scene() -> (Scene, MeshId) {
    let mut scene = Scene::default();
    let mesh = scene
        .resources
        .insert_mesh(MeshSource::new(MeshDescriptor::triangles(6, 0), |_| {
            panic!("validation must not prepare a mesh")
        }))
        .expect("fresh mesh");
    let texture = scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            |_| panic!("validation must not prepare a texture"),
        ))
        .expect("fresh texture");
    let coverage = scene
        .resources
        .insert_mask(MaskSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
            |_| panic!("validation must not prepare coverage"),
        ))
        .expect("fresh coverage");
    for _ in 0..32 {
        scene.pixel_masks.push(PixelMask {
            mesh,
            texture: coverage,
            transform: Matrix4::identity(),
            parent: None,
        });
    }
    scene.phases.push(Phase {
        objects: (0..8_000)
            .map(|_| Object {
                mask: Some(PixelMaskIndex(0)),
                ..Object::new(mesh, texture, Matrix4::identity())
            })
            .collect(),
    });
    (scene, mesh)
}

fn check(scene: &Scene, scratch: &mut ValidationScratch) -> Result<(), SceneError> {
    validate_scene(
        scene,
        &wgpu::Limits::default(),
        wgpu::Features::empty(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        scratch,
    )
}

#[test]
fn repeated_resource_ids_share_validation_storage_across_frames() {
    let (scene, _) = repeated_scene();
    let mut scratch = ValidationScratch::default();
    check(&scene, &mut scratch).expect("repeated descriptors are valid");
    assert_eq!(
        (
            scratch.meshes.len(),
            scratch.textures.len(),
            scratch.masks.len()
        ),
        (1, 1, 1)
    );
    let capacities = (
        scratch.meshes.capacity(),
        scratch.textures.capacity(),
        scratch.masks.capacity(),
    );
    check(&scene, &mut scratch).expect("next frame validates again");
    assert_eq!(
        (
            scratch.meshes.capacity(),
            scratch.textures.capacity(),
            scratch.masks.capacity()
        ),
        capacities
    );
}

#[test]
fn repeated_ids_do_not_skip_object_properties_or_mask_topology() {
    let (mut scene, _) = repeated_scene();
    let mut scratch = ValidationScratch::default();
    scene.phases[0]
        .objects
        .last_mut()
        .expect("many objects")
        .opacity = f32::NAN;
    assert!(check(&scene, &mut scratch).is_err());
    scene.phases[0]
        .objects
        .last_mut()
        .expect("many objects")
        .opacity = 1.;
    check(&scene, &mut scratch).expect("opacity repaired");
    scene.phases[0]
        .objects
        .last_mut()
        .expect("many objects")
        .transform[(0, 0)] = f32::INFINITY;
    assert!(check(&scene, &mut scratch).is_err());
    scene.phases[0]
        .objects
        .last_mut()
        .expect("many objects")
        .transform = Matrix4::identity();
    scene.pixel_masks[31].transform[(0, 0)] = f32::NAN;
    assert!(check(&scene, &mut scratch).is_err());
    scene.pixel_masks[31].transform = Matrix4::identity();
    scene.pixel_masks[31].parent = Some(PixelMaskIndex(31));
    assert!(check(&scene, &mut scratch).is_err());
    scene.pixel_masks[31].parent = None;
    check(&scene, &mut scratch).expect("topology repaired");
}

#[test]
fn previously_seen_ids_are_revalidated_after_scene_definitions_change() {
    let (mut scene, mesh) = repeated_scene();
    let mut scratch = ValidationScratch::default();
    check(&scene, &mut scratch).expect("original descriptor");
    scene.resources.retain_meshes(|_| false);
    scene
        .resources
        .insert_mesh(MeshSource::with_id(
            mesh,
            MeshDescriptor::triangles(4, 0),
            |_| panic!("invalid descriptor must not run"),
        ))
        .expect("replacement definition in a later scene");
    assert!(check(&scene, &mut scratch).is_err());
    scene.resources.retain_meshes(|_| false);
    assert!(check(&scene, &mut scratch).is_err());
}

#[test]
fn unused_mask_nodes_still_require_valid_definitions() {
    let (mut scene, mesh) = repeated_scene();
    let invalid = scene
        .resources
        .insert_mask(MaskSource::new(
            TextureDescriptor::new([0, 1], wgpu::TextureFormat::R8Unorm),
            |_| panic!("unused invalid mask must not run"),
        ))
        .expect("fresh invalid coverage definition");
    scene.pixel_masks.push(PixelMask {
        mesh,
        texture: invalid,
        transform: Matrix4::identity(),
        parent: None,
    });
    assert!(check(&scene, &mut ValidationScratch::default()).is_err());
}
