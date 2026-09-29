//! Device/descriptor checks precede recording, including warm-cache submissions.
use crate::{
    PlainError, PlainTarget,
    resources::{Entry, Image, Mesh, ResourceStore, ValidationScratch},
};
use render_interface::*;
use std::collections::HashMap;

#[cfg(test)]
mod dedup_tests;

pub(crate) fn validate(
    device: &wgpu::Device,
    resources: &mut ResourceStore,
    scene: &Scene,
    target: &PlainTarget<'_>,
) -> Result<(), PlainError> {
    let invalid = |msg: &str| PlainError::Invalid(msg.into());
    // TextureRegion's constructor validates subresource metadata, format
    // compatibility and nonempty in-bounds extents. Usage is role-specific here.
    let destination = target.region.texture();
    validate_target_format(destination.format(), target.region.view_format())?;
    if [
        target.clear.r,
        target.clear.g,
        target.clear.b,
        target.clear.a,
    ]
    .iter()
    .any(|v| !v.is_finite())
        || !(0.0..=1.0).contains(&target.clear.a)
    {
        return Err(invalid("invalid initial clear colour"));
    }
    if let Some(initial) = target.initial {
        let texture = initial.texture();
        if texture == destination
            || initial.size() != target.region.size()
            || !texture
                .usage()
                .contains(wgpu::TextureUsages::TEXTURE_BINDING)
        {
            return Err(invalid(
                "initial region must have the destination extent and a separate sampled texture",
            ));
        }
        if !matches!(
            initial.view_format(),
            wgpu::TextureFormat::R8Unorm
                | wgpu::TextureFormat::Rgba8Unorm
                | wgpu::TextureFormat::Rgba8UnormSrgb
                | wgpu::TextureFormat::Bgra8Unorm
                | wgpu::TextureFormat::Bgra8UnormSrgb
                | wgpu::TextureFormat::Rgba16Float
        ) {
            return Err(invalid("unsupported initial image format"));
        }
    }
    if target
        .logical_size
        .iter()
        .any(|v| !v.is_finite() || *v <= 0.)
    {
        return Err(invalid("logical scene size must be finite and positive"));
    }
    if !destination
        .usage()
        .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
    {
        return Err(invalid("destination must be a render attachment"));
    }
    validate_scene(
        scene,
        &device.limits(),
        device.features(),
        &resources.meshes,
        &resources.textures,
        &resources.masks,
        &mut resources.validation,
    )
}

/// Descriptor checks depend on resource identity, while transform/opacity and
/// mask topology belong to individual occurrences. Keep the latter checks even
/// for repeated occurrences of one ID. Scratch membership lasts for this call only.
fn validate_scene(
    scene: &Scene,
    limits: &wgpu::Limits,
    features: wgpu::Features,
    meshes: &HashMap<MeshId, Entry<Mesh>>,
    textures: &HashMap<TextureId, Entry<Image>>,
    masks: &HashMap<MaskId, Entry<Image>>,
    scratch: &mut ValidationScratch,
) -> Result<(), PlainError> {
    scratch.clear();
    let invalid = |msg: &str| PlainError::Invalid(msg.into());
    let check_mesh = |id| -> Result<(), PlainError> {
        let source = scene
            .resources
            .mesh(id)
            .ok_or_else(|| invalid("missing mesh definition"))?;
        let d = source.descriptor();
        if let Some([min, max]) = d.bounds
            && (0..3).any(|i| !min[i].is_finite() || !max[i].is_finite() || min[i] > max[i])
        {
            return Err(invalid("invalid conservative mesh bounds"));
        }
        if d.vertex_count == 0
            || (if d.index_count == 0 {
                d.vertex_count
            } else {
                d.index_count
            }) % 3
                != 0
        {
            return Err(invalid("triangle mesh has invalid counts"));
        }
        if u64::from(d.vertex_count) * 20 > limits.max_buffer_size
            || u64::from(d.index_count) * 4 > limits.max_buffer_size
        {
            return Err(invalid("mesh exceeds device buffer limit"));
        }
        validate_mesh_usages(d.usages)?;
        if meshes.get(&id).is_some_and(|e| e.value.desc != *d) {
            return Err(invalid("cached mesh ID changed descriptor"));
        }
        Ok(())
    };
    let check_image = |d: &TextureDescriptor| -> Result<(), PlainError> {
        validate_texture_usages(d.usages)?;
        if d.size
            .iter()
            .any(|v| *v == 0 || *v > limits.max_texture_dimension_2d)
        {
            return Err(invalid("texture size exceeds device limits or is zero"));
        }
        if !matches!(
            d.format,
            wgpu::TextureFormat::R8Unorm
                | wgpu::TextureFormat::Rgba8Unorm
                | wgpu::TextureFormat::Rgba8UnormSrgb
                | wgpu::TextureFormat::Rgba16Float
        ) {
            return Err(invalid("unsupported sampled texture format"));
        }
        let usages = d.usages
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC;
        if !d
            .format
            .guaranteed_format_features(features)
            .allowed_usages
            .contains(usages)
        {
            return Err(invalid("texture format does not support requested usages"));
        }
        Ok(())
    };
    for (i, m) in scene.pixel_masks.iter().enumerate() {
        if m.parent.is_some_and(|p| p.0 as usize >= i) {
            return Err(invalid("mask parent must precede child"));
        }
        if m.transform.iter().any(|x| !x.is_finite()) {
            return Err(invalid("nonfinite mask transform"));
        }
        if scratch.meshes.insert(m.mesh) {
            check_mesh(m.mesh)?;
        }
        if scratch.masks.insert(m.texture) {
            let d = scene
                .resources
                .mask(m.texture)
                .ok_or_else(|| invalid("missing mask definition"))?
                .descriptor();
            check_image(d)?;
            if masks.get(&m.texture).is_some_and(|e| e.value.desc != *d) {
                return Err(invalid("cached mask ID changed descriptor"));
            }
        }
    }
    for o in scene.phases.iter().flat_map(|p| &p.objects) {
        if o.mask
            .is_some_and(|p| p.0 as usize >= scene.pixel_masks.len())
        {
            return Err(invalid("object mask index out of bounds"));
        }
        if !o.opacity.is_finite()
            || !(0.0..=1.0).contains(&o.opacity)
            || o.transform.iter().any(|x| !x.is_finite())
        {
            return Err(invalid("invalid object transform/opacity"));
        }
        if scratch.meshes.insert(o.mesh) {
            check_mesh(o.mesh)?;
        }
        if scratch.textures.insert(o.texture) {
            let d = scene
                .resources
                .texture(o.texture)
                .ok_or_else(|| invalid("missing texture definition"))?
                .descriptor();
            check_image(d)?;
            if textures.get(&o.texture).is_some_and(|e| e.value.desc != *d) {
                return Err(invalid("cached texture ID changed descriptor"));
            }
        }
    }
    Ok(())
}

fn validate_target_format(
    texture_format: wgpu::TextureFormat,
    view_format: wgpu::TextureFormat,
) -> Result<(), PlainError> {
    if !matches!(
        view_format,
        wgpu::TextureFormat::Rgba8Unorm
            | wgpu::TextureFormat::Rgba8UnormSrgb
            | wgpu::TextureFormat::Bgra8Unorm
            | wgpu::TextureFormat::Bgra8UnormSrgb
            | wgpu::TextureFormat::Rgba16Float
    ) {
        return Err(PlainError::Invalid("unsupported destination format".into()));
    }
    // wgpu does not expose a view's descriptor. The caller supplies its actual
    // format; verify the compatible format family without pretending to inspect
    // the view. View creation itself validates that reinterpretation was enabled.
    if view_format.remove_srgb_suffix() != texture_format.remove_srgb_suffix() {
        return Err(PlainError::Invalid(
            "destination view format is incompatible with its texture".into(),
        ));
    }
    Ok(())
}

fn validate_mesh_usages(usages: wgpu::BufferUsages) -> Result<(), PlainError> {
    let supported = wgpu::BufferUsages::COPY_SRC
        | wgpu::BufferUsages::COPY_DST
        | wgpu::BufferUsages::VERTEX
        | wgpu::BufferUsages::INDEX
        | wgpu::BufferUsages::UNIFORM
        | wgpu::BufferUsages::STORAGE
        | wgpu::BufferUsages::INDIRECT
        | wgpu::BufferUsages::QUERY_RESOLVE;
    // Outputs are not mappable. Ray-tracing inputs and unknown/future flags are
    // deliberately outside this backend's supported preparation capabilities.
    if !supported.contains(usages) {
        return Err(PlainError::Invalid("unsupported mesh output usage".into()));
    }
    Ok(())
}

fn validate_texture_usages(usages: wgpu::TextureUsages) -> Result<(), PlainError> {
    if !wgpu::TextureUsages::all().contains(usages) {
        return Err(PlainError::Invalid("unknown texture output usage".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop_device() -> wgpu::Device {
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
                .0
        })
    }

    fn view(device: &wgpu::Device, size: [u32; 2]) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("region validation fixture"),
                size: crate::resources::extent(size),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    // NOOP checks only the CPU contract: pixel preservation and filtering have
    // separate real-backend integration tests.
    #[test]
    fn initial_regions_match_extent_independently_of_texture_size_and_origin() {
        let device = noop_device();
        let destination = view(&device, [32, 16]);
        let initial = view(&device, [64, 64]);
        let region = |view, origin, size| {
            TextureRegion::new(view, wgpu::TextureFormat::Rgba8Unorm, origin, size)
                .expect("valid fixture region")
        };
        let mut resources = ResourceStore::new(&device);
        let scene = Scene::default();
        let mut target = PlainTarget {
            region: region(&destination, [3, 5], [8, 4]),
            logical_size: [80., 40.],
            clear: wgpu::Color::TRANSPARENT,
            initial: Some(region(&initial, [17, 23], [8, 4])),
        };
        assert!(validate(&device, &mut resources, &scene, &target).is_ok());

        target.initial = Some(region(&initial, [17, 23], [7, 4]));
        assert!(validate(&device, &mut resources, &scene, &target).is_err());

        // Even disjoint rectangles of one texture are conservatively rejected.
        target.initial = Some(region(&destination, [16, 0], [8, 4]));
        assert!(validate(&device, &mut resources, &scene, &target).is_err());
    }

    #[test]
    fn output_view_formats_allow_srgb_reinterpretation_but_not_other_families() {
        use wgpu::TextureFormat::*;
        assert!(validate_target_format(Rgba8Unorm, Rgba8UnormSrgb).is_ok());
        assert!(validate_target_format(Bgra8UnormSrgb, Bgra8Unorm).is_ok());
        assert!(validate_target_format(Rgba16Float, Rgba16Float).is_ok());
        assert!(validate_target_format(Rgba8Unorm, Bgra8Unorm).is_err());
        assert!(validate_target_format(Rgba8Unorm, Rgba16Float).is_err());
        assert!(validate_target_format(R8Unorm, R8Unorm).is_err());
    }

    #[test]
    fn mesh_usage_support_excludes_mapping_ray_tracing_and_unknown_bits() {
        use wgpu::BufferUsages as Usage;
        assert!(validate_mesh_usages(Usage::empty()).is_ok());
        assert!(validate_mesh_usages(Usage::STORAGE | Usage::COPY_SRC).is_ok());
        for unsupported in [
            Usage::MAP_READ,
            Usage::MAP_WRITE,
            Usage::BLAS_INPUT,
            Usage::TLAS_INPUT,
            Usage::from_bits_retain(1 << 31),
        ] {
            assert!(validate_mesh_usages(Usage::COPY_DST | unsupported).is_err());
        }
    }

    #[test]
    fn unknown_texture_usages_are_rejected_before_device_checks() {
        use wgpu::TextureUsages as Usage;
        assert!(validate_texture_usages(Usage::empty()).is_ok());
        assert!(validate_texture_usages(Usage::STORAGE_BINDING | Usage::RENDER_ATTACHMENT).is_ok());
        assert!(validate_texture_usages(Usage::from_bits_retain(1 << 31)).is_err());
    }
}
