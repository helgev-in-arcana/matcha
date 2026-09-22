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
