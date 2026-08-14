//! How [`FrameParams`] reaches the shaders.
//!
//! Natively it is an immediate (a push constant): free to set, no buffer, no
//! bind group. WebGPU has no such thing, so the web build puts the identical
//! block in a uniform buffer bound at group [`PARAMS_GROUP`] instead.
//!
//! Both paths share one set of WGSL sources. Rather than fork five shaders,
//! [`prepare_wgsl`] rewrites the single declaration line at pipeline-creation
//! time — the shaders are `include_str!` consts handed to
//! `ShaderSource::Wgsl(Cow<str>)`, so this costs one string scan per pipeline
//! and keeps one source of truth for the struct.
//!
//! The uniform path is selected by `web` *or* the `uniform-params` feature. The
//! feature exists so this code can be run against a real desktop GPU: a WGSL
//! uniform block is perfectly legal natively, so `cargo test -p renderer
//! --features uniform-params` exercises the shader rewrite, the bind group
//! index, the buffer size and the uniform-layout rules with no browser
//! involved. That matters because a mistake here shows up in a browser as a
//! black canvas and a console validation error, and nothing else.

use super::FrameParams;

/// Bind group index the uniform-backed parameter block occupies.
///
/// Free in every pipeline in this renderer: the render pass uses 0 (textures)
/// and 1 (data), the Blelloch scan uses 0 (data) and 1 (block sums), and every
/// other compute stage uses 0 alone.
#[cfg(any(web, feature = "uniform-params"))]
pub(crate) const PARAMS_GROUP: u32 = 2;

/// True when the parameter block travels in a uniform buffer rather than as an
/// immediate.
pub(crate) const USES_UNIFORM: bool = cfg!(any(web, feature = "uniform-params"));

/// Adapt a shader source to the active parameter-passing path.
///
/// Identity on the immediate path. On the uniform path, rewrites
/// `var<immediate> pc: Pc;` into a uniform binding at [`PARAMS_GROUP`].
///
/// The rewrite is a plain substring replacement, which is only safe because
/// every shader spells that declaration identically. A shader that reformats
/// or renames the declaration keeps its push constant, and then fails — a
/// browser rejects the WGSL at `createShaderModule` (wgpu's WebGPU backend
/// hands the source straight over, so naga never sees it), and under
/// `uniform-params` naga rejects it natively against an `immediate_size` of 0.
/// Loud, but only once a pipeline is built, and on the web only in a console.
/// `every_shader_declares_the_parameter_block_identically` below catches it in
/// the default `cargo test` run instead, with no GPU and no feature.
pub(crate) fn prepare_wgsl(src: &str) -> std::borrow::Cow<'_, str> {
    #[cfg(any(web, feature = "uniform-params"))]
    {
        let out = src.replace(
            "var<immediate> pc: Pc;",
            &format!("@group({PARAMS_GROUP}) @binding(0) var<uniform> pc: Pc;"),
        );
        debug_assert!(
            !out.contains("var<immediate>"),
            "a shader still declares `var<immediate>` after the uniform rewrite; the \
             declaration must be spelled exactly `var<immediate> pc: Pc;` (see \
             core_renderer/frame_params.rs)"
        );
        std::borrow::Cow::Owned(out)
    }
    #[cfg(not(any(web, feature = "uniform-params")))]
    {
        std::borrow::Cow::Borrowed(src)
    }
}

/// Owns whatever the active path needs to get [`FrameParams`] to a pipeline.
///
/// On the immediate path this is a zero-sized token and every method is a
/// direct `set_immediates`. On the uniform path it owns the buffer, its bind
/// group layout and its bind group.
pub(crate) struct FrameParamsBinding {
    #[cfg(any(web, feature = "uniform-params"))]
    buffer: wgpu::Buffer,
    #[cfg(any(web, feature = "uniform-params"))]
    layout: wgpu::BindGroupLayout,
    #[cfg(any(web, feature = "uniform-params"))]
    bind_group: wgpu::BindGroup,
}

impl FrameParamsBinding {
    #[cfg(any(web, feature = "uniform-params"))]
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjectRenderer Frame Params Buffer"),
            size: std::mem::size_of::<FrameParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ObjectRenderer Frame Params Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                // Every stage of both pipelines reads this block.
                visibility: wgpu::ShaderStages::COMPUTE
                    | wgpu::ShaderStages::VERTEX
                    | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjectRenderer Frame Params Bind Group"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            buffer,
            layout,
            bind_group,
        }
    }

    #[cfg(not(any(web, feature = "uniform-params")))]
    pub(crate) fn new(_device: &wgpu::Device) -> Self {
        Self {}
    }

    /// Bytes of immediate storage a pipeline layout must reserve: the block on
    /// the immediate path, nothing on the uniform path.
    pub(crate) fn immediate_size(&self) -> u32 {
        if USES_UNIFORM {
            0
        } else {
            std::mem::size_of::<FrameParams>() as u32
        }
    }

    /// Extend a pipeline's bind group layouts with this binding's own, if the
    /// active path has one.
    ///
    /// Returns `base` unchanged on the immediate path, rather than appending a
    /// trailing `None`, so pipeline layouts stay exactly as they were.
    pub(crate) fn extend_layouts<'a>(
        &'a self,
        base: &[Option<&'a wgpu::BindGroupLayout>],
    ) -> Vec<Option<&'a wgpu::BindGroupLayout>> {
        let mut layouts = base.to_vec();
        #[cfg(any(web, feature = "uniform-params"))]
        {
            // `base` covers groups 0..n; pad so this lands exactly on
            // PARAMS_GROUP even when a pipeline leaves an index unused.
            layouts.resize(PARAMS_GROUP as usize, None);
            layouts.push(Some(&self.layout));
        }
        layouts
    }

    /// Upload the frame's parameters. Call once per frame, before encoding.
    /// A no-op on the immediate path, where the values travel with each pass.
    pub(crate) fn write(&self, _queue: &wgpu::Queue, _params: &FrameParams) {
        #[cfg(any(web, feature = "uniform-params"))]
        _queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(_params));
    }

    /// Make the parameters visible to a compute pass.
    pub(crate) fn set_compute(&self, pass: &mut wgpu::ComputePass<'_>, _params: &FrameParams) {
        #[cfg(any(web, feature = "uniform-params"))]
        pass.set_bind_group(PARAMS_GROUP, &self.bind_group, &[]);
        #[cfg(not(any(web, feature = "uniform-params")))]
        pass.set_immediates(0, bytemuck::bytes_of(_params));
    }

    /// Make the parameters visible to a render pass.
    pub(crate) fn set_render(&self, pass: &mut wgpu::RenderPass<'_>, _params: &FrameParams) {
        #[cfg(any(web, feature = "uniform-params"))]
        pass.set_bind_group(PARAMS_GROUP, &self.bind_group, &[]);
        #[cfg(not(any(web, feature = "uniform-params")))]
        pass.set_immediates(0, bytemuck::bytes_of(_params));
    }
}

#[cfg(test)]
mod tests {
    /// Every shader that reads [`FrameParams`], with the shaders' own names,
    /// so a failure says which file to look at.
    const DECLARING: &[(&str, &str)] = &[
        ("renderer_render.wgsl", super::super::WGSL_RENDER),
        ("renderer_cull.wgsl", super::super::stages::WGSL_CULL),
        (
            "renderer_prefix_sum_blelloch.wgsl",
            super::super::stages::WGSL_PREFIX_SUM_BLELLOCH,
        ),
        (
            "renderer_prefix_sum_single_thread.wgsl",
            super::super::stages::WGSL_PREFIX_SUM_SINGLE_THREAD,
        ),
        ("renderer_scatter.wgsl", super::super::stages::WGSL_SCATTER),
    ];

    /// [`prepare_wgsl`](super::prepare_wgsl) rewrites the parameter block's
    /// declaration by exact substring match, so a shader that reformats or
    /// renames it keeps a push constant the web cannot use.
    ///
    /// Checked as text rather than by asserting on the rewrite's output,
    /// because that is the form the check works in on *both* paths: on the
    /// immediate path `prepare_wgsl` is the identity and has nothing to
    /// assert. This is the whole reason the check can live in the default test
    /// run rather than behind `uniform-params` or a browser.
    #[test]
    fn every_shader_declares_the_parameter_block_identically() {
        for (name, src) in DECLARING {
            assert_eq!(
                src.matches("var<immediate> pc: Pc;").count(),
                1,
                "{name} must declare the frame parameters exactly as \
                 `var<immediate> pc: Pc;`, on one line, once"
            );
        }
    }

    /// The command stage reads no parameters. It still gets the binding in its
    /// pipeline layout (every group must be bound before a dispatch), but it
    /// must not declare one — nothing would rewrite it, since the pipeline
    /// reserves no immediate storage.
    #[test]
    fn the_command_shader_declares_no_parameter_block() {
        assert!(!super::super::stages::WGSL_COMMAND.contains("var<immediate>"));
    }
}
