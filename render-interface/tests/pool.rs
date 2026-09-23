use render_interface::*;

#[test]
fn duplicate_registration_is_rejected_without_replacing_original() {
    let mut pool = ResourcePool::default();
    let desc = TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm);
    let first = TextureSource::new(desc, |_| Ok(()));
    let id = first.id();
    pool.insert_texture(first).expect("first definition");
    let different = TextureDescriptor::new([2, 2], wgpu::TextureFormat::Rgba8Unorm);
    assert!(
        pool.insert_texture(TextureSource::with_id(id, different, |_| Ok(())))
            .is_err()
    );
    assert_eq!(
        *pool.texture(id).expect("original remains").descriptor(),
        desc
    );
    assert_eq!(pool.len(), 1);
}

#[test]
fn ids_are_unique_across_resource_types_and_concurrent_allocation() {
    let handles: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                (0..500)
                    .flat_map(|_| {
                        [
                            MeshId::new().get(),
                            TextureId::new().get(),
                            MaskId::new().get(),
                        ]
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let ids: std::collections::HashSet<_> = handles
        .into_iter()
        .flat_map(|h| h.join().expect("ID allocator thread"))
        .collect();
    assert_eq!(ids.len(), 6000);
}

#[test]
fn composing_definitions_uses_content_identity_not_closure_pointer_identity() {
    let desc = TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm);
    let id = TextureId::new();
    let mut a = ResourcePool::default();
    let mut b = ResourcePool::default();
    a.insert_texture(TextureSource::with_id(id, desc, |_| Ok(())))
        .expect("first definition");
    b.insert_texture(TextureSource::with_id(id, desc, |_| Ok(())))
        .expect("same content reconstructed independently");
    a.import(&b)
        .expect("caller promises same logical content for this ID");
    assert_eq!(a.len(), 1);
    assert!(
        a.insert_texture(TextureSource::with_id(id, desc, |_| Ok(())))
            .is_err(),
        "direct duplicate submissions remain errors"
    );
}

#[test]
fn conflicting_import_leaves_all_resource_types_unchanged() {
    let mut destination = ResourcePool::default();
    let mut incoming = ResourcePool::default();
    let desc = TextureDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm);
    let mask = MaskSource::new(desc, |_| panic!("registration must not prepare resources"));
    let mask_id = mask.id();
    destination.insert_mask(mask).expect("original mask");
    incoming
        .insert_mask(MaskSource::with_id(
            mask_id,
            TextureDescriptor::new([2, 2], desc.format),
            |_| panic!("registration must not prepare resources"),
        ))
        .expect("conflicting definition in a separate pool");
    let mesh_id = incoming
        .insert_mesh(MeshSource::new(MeshDescriptor::triangles(3, 0), |_| {
            panic!("registration must not prepare resources")
        }))
        .expect("new mesh");

    assert!(destination.import(&incoming).is_err());
    assert_eq!(destination.len(), 1);
    assert!(destination.mesh(mesh_id).is_none());
    assert_eq!(
        *destination
            .mask(mask_id)
            .expect("original mask")
            .descriptor(),
        desc
    );
}

#[test]
fn sharing_and_retention_do_not_prepare_or_invalidate_provider_definitions() {
    let source = MeshSource::new(MeshDescriptor::triangles(3, 0), |_| {
        panic!("pool operations must not prepare resources")
    });
    let mut pool = ResourcePool::default();
    assert_eq!(pool.share_mesh(&source).expect("first share"), source.id());
    pool.share_mesh(&source).expect("repeated share");
    assert_eq!(pool.mesh_ids().collect::<Vec<_>>(), vec![source.id()]);
    pool.retain_meshes(|_| false);
    assert!(pool.is_empty());
    pool.share_mesh(&source)
        .expect("provider definition remains reusable");
    assert_eq!(pool.len(), 1);
}
