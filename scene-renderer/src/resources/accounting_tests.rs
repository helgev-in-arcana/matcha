//! CPU residency bookkeeping tests use wgpu's noop backend; no physical GPU,
//! pixel readback or timing claims are involved.
use super::*;

fn context() -> (wgpu::Device, wgpu::Queue) {
    futures::executor::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::NOOP,
            backend_options: wgpu::BackendOptions {
                noop: wgpu::NoopBackendOptions { enable: true },
                ..Default::default()
            },
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        instance
            .request_adapter(&Default::default())
            .await
            .expect("noop adapter")
            .request_device(&Default::default())
            .await
            .expect("noop device")
    })
}

struct Fixture {
    scene: Scene,
    mesh: MeshId,
    texture: TextureId,
    mask: MaskId,
}

impl Fixture {
    fn new() -> Self {
        let mut scene = Scene::default();
        let mesh = scene
            .resources
            .insert_mesh(MeshSource::new(MeshDescriptor::triangles(6, 3), |ctx| {
                ctx.gpu.encoder.clear_buffer(ctx.target.vertices, 0, None);
                ctx.gpu
                    .encoder
                    .clear_buffer(ctx.target.indices.expect("indexed fixture"), 0, None);
                Ok(())
            }))
            .expect("fresh mesh");
        let texture = scene
            .resources
            .insert_texture(TextureSource::new(
                TextureDescriptor::new([2, 2], wgpu::TextureFormat::Rgba8Unorm),
                |mut ctx| upload_texture(&mut ctx.gpu, &ctx.target, &[255; 16]),
            ))
            .expect("fresh texture");
        let mask = scene
            .resources
            .insert_mask(MaskSource::new(
                TextureDescriptor::new([2, 2], wgpu::TextureFormat::R8Unorm),
                |mut ctx| upload_texture(&mut ctx.gpu, &ctx.target, &[255; 4]),
            ))
            .expect("fresh mask");
        scene.pixel_masks.push(PixelMask {
            mesh,
            texture: mask,
            transform: Matrix4::identity(),
            parent: None,
        });
        scene.phases.push(Phase {
            objects: vec![Object {
                mask: Some(PixelMaskIndex(0)),
                ..Object::new(mesh, texture, Matrix4::identity())
            }],
        });
        Self {
            scene,
            mesh,
            texture,
            mask,
        }
    }

    fn prepare(&self, store: &mut ResourceStore, device: &wgpu::Device) -> wgpu::CommandEncoder {
        let image = make_image(
            device,
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
        );
        let snapshot = RenderSnapshot {
            color_texture: &image.texture,
            color_view: &image.view,
            size: [1, 1],
            format: image.desc.format,
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        store
            .prepare_mesh(&self.scene, self.mesh, &mut encoder, snapshot)
            .expect("prepare mesh");
        store
            .prepare_texture(&self.scene, self.texture, &mut encoder, snapshot)
            .expect("prepare texture");
        store
            .prepare_mask(&self.scene, self.mask, &mut encoder, snapshot)
            .expect("prepare mask");
        encoder
    }
}

/// Independent slow oracle, intentionally scanning every entry. Production stats
/// must agree with it after each mutation without paying this cost on warm frames.
fn assert_accounting(store: &ResourceStore) {
    let expected_bytes = store.meshes.values().map(|entry| entry.bytes).sum::<u64>()
        + store
            .textures
            .values()
            .chain(store.masks.values())
            .map(|entry| entry.bytes)
            .sum::<u64>();
    assert_eq!(store.cache_bytes(), expected_bytes);
    let mut expected = store.placement.stats();
    for entry in store.textures.values().chain(store.masks.values()) {
        if entry.value.texture_lease.is_none() {
            expected.texture_pages += 1;
            expected.reserved_texture_bytes += entry.bytes;
            expected.live_texture_bytes += entry.bytes;
        }
    }
    for entry in store.meshes.values() {
        let mesh = &entry.value;
        if mesh.vertex_lease.is_none() {
            expected.mesh_pages += 1;
            expected.reserved_mesh_bytes += mesh.vertices.size();
            expected.live_mesh_bytes += mesh.vertex_range.end - mesh.vertex_range.start;
        }
        if let Some(indices) = &mesh.indices
            && mesh.index_lease.is_none()
        {
            expected.mesh_pages += 1;
            expected.reserved_mesh_bytes += indices.size();
            expected.live_mesh_bytes += mesh.index_range.end - mesh.index_range.start;
        }
    }
    assert_eq!(store.placement_stats(), expected);
}

#[test]
fn incremental_totals_survive_insert_abort_eviction_clear_and_relocation() {
    let (device, queue) = context();
    for mode in [PlacementMode::Atlas, PlacementMode::Dedicated] {
        let mut store = ResourceStore::new(&device);
        store.set_mode(mode);
        store
            .set_config(AtlasConfig {
                texture_edge: 8,
                mesh_page_bytes: 64,
            })
            .expect("small valid pages");
        let original = Fixture::new();
        let other = Fixture::new();
        store.begin().expect("begin original frame");
        let encoder = original.prepare(&mut store, &device);
        assert_eq!(store.cache_bytes(), 152);
        assert_accounting(&store);
        queue.submit([encoder.finish()]);
        store.finish(&original.scene);

        store.begin().expect("begin aborted frame");
        let encoder = other.prepare(&mut store, &device);
        assert_eq!(store.cache_bytes(), 304);
        assert_accounting(&store);
        drop(encoder);
        store.abort();
        assert_eq!(store.cache_bytes(), 152);
        assert_accounting(&store);

        let before = store.placement_stats();
        let mut encoder = device.create_command_encoder(&Default::default());
        assert!(store.plan_relocation(&mut encoder, 0).is_err());
        assert_eq!(store.placement_stats(), before);
        assert_accounting(&store);
        let plan = store
            .plan_relocation(&mut encoder, u64::MAX)
            .expect("relocation fits");
        queue.submit([encoder.finish()]);
        store.commit_relocation(plan);
        assert_eq!(store.cache_bytes(), 152);
        assert_accounting(&store);

        store.set_budget(152);
        store.begin().expect("begin replacement frame");
        let encoder = other.prepare(&mut store, &device);
        queue.submit([encoder.finish()]);
        store.finish(&other.scene);
        assert_eq!(store.cache_bytes(), 152);
        assert_eq!(store.stats.evicted, 3);
        assert!(!store.meshes.contains_key(&original.mesh));
        assert!(store.meshes.contains_key(&other.mesh));
        assert_accounting(&store);

        store.clear();
        assert_eq!(store.cache_bytes(), 0);
        assert_eq!(store.placement_stats(), PlacementStats::default());
        assert_accounting(&store);
        store.begin().expect("begin after clear");
        let encoder = original.prepare(&mut store, &device);
        queue.submit([encoder.finish()]);
        store.finish(&original.scene);
        store.set_mode(if mode == PlacementMode::Atlas {
            PlacementMode::Dedicated
        } else {
            PlacementMode::Atlas
        });
        assert_eq!(store.cache_bytes(), 0);
        assert_accounting(&store);
        store.begin().expect("begin after mode change");
        let encoder = other.prepare(&mut store, &device);
        queue.submit([encoder.finish()]);
        store.finish(&other.scene);
        store
            .set_config(AtlasConfig {
                texture_edge: 16,
                mesh_page_bytes: 128,
            })
            .expect("new valid page configuration");
        assert_eq!(store.cache_bytes(), 0);
        assert_accounting(&store);
    }
}
