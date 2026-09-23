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
