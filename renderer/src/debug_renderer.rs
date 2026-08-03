use crate::render_node::RenderNode;
use std::sync::Arc;

/// A very small debug renderer that draws a single layer of the provided atlas
/// as a fullscreen quad. API mirrors `CoreRenderer::render`.
pub struct DebugRenderer {
    texture_sampler: wgpu::Sampler,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    shader_module: wgpu::ShaderModule,
    render_pipeline_cache:
        crate::pipeline_cache::PipelineCache<wgpu::TextureFormat, Arc<wgpu::RenderPipeline>>,
}

impl DebugRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("DebugRenderer Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("DebugRenderer Texture Bind Group Layout"),
                entries: &[
                    // sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    // texture array
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2Array,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });

        // shader will be created per-instance (we keep a module here but the layer
        // index is baked into the shader at creation time in `render`)
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("DebugRenderer Shader (placeholder)"),
            source: wgpu::ShaderSource::Wgsl("".into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("DebugRenderer Pipeline Layout"),
            bind_group_layouts: &[Some(&texture_bind_group_layout)],
            immediate_size: 0,
        });

        let render_pipeline_cache = crate::pipeline_cache::PipelineCache::new(4);

        Self {
            texture_sampler,
            texture_bind_group_layout,
            pipeline_layout,
            shader_module,
            render_pipeline_cache,
        }
    }

    fn create_render_pipeline(
        &self,
        device: &wgpu::Device,
        shader_module: &wgpu::ShaderModule,
        target_format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("DebugRenderer Pipeline"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader_module,
                entry_point: Some("vertex_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: shader_module,
                entry_point: Some("fragment_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    }

    /// Render the given atlas (specified layer) fullscreen. Mirrors CoreRenderer::render signature.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        destination_view: &wgpu::TextureView,
        destination_size: [f32; 2],
        _objects: &RenderNode,
        load_color: wgpu::Color,
        texture_atlas: &wgpu::Texture,
        _stencil_atlas: &wgpu::Texture,
    ) -> Result<(), crate::core_renderer::TextureValidationError> {
        // Hardcoded atlas layer to display (change as needed).
        const DEBUG_ATLAS_LAYER: u32 = 0;

        // Build shader source with baked layer index.
        let shader_source = format!(
            r#"
struct VertexOutput {{
    @builtin(position) pos : vec4<f32>,
    @location(0) uv : vec2<f32>,
}};

@vertex
fn vertex_main(@builtin(vertex_index) v_idx : u32) -> VertexOutput {{
    var out : VertexOutput;
    let positions = array<vec2<f32>,4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0)
    );
    let uvs = array<vec2<f32>,4>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0)
    );
    out.pos = vec4<f32>(positions[v_idx], 0.0, 1.0);
    out.uv = uvs[v_idx];
    return out;
}}

@group(0) @binding(0) var samp : sampler;
@group(0) @binding(1) var tex : texture_2d_array<f32>;

@fragment
fn fragment_main(in_ : VertexOutput) -> @location(0) vec4<f32> {{
    // sample hardcoded layer
    let col = textureSample(tex, samp, in_.uv, {DEBUG_ATLAS_LAYER});
    return col;
}}
"#
        );

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("DebugRenderer Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let pipeline = self.render_pipeline_cache.get_with(surface_format, || {
            Arc::new(self.create_render_pipeline(device, &shader_module, surface_format))
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("DebugRenderer Texture Bind Group"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&self.texture_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&texture_atlas.create_view(
                        &wgpu::TextureViewDescriptor {
                            dimension: Some(wgpu::TextureViewDimension::D2Array),
                            aspect: wgpu::TextureAspect::All,
                            ..Default::default()
                        },
                    )),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("DebugRenderer Command Encoder"),
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("DebugRenderer Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: destination_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(load_color),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });

            rpass.set_pipeline(pipeline.as_ref());
            rpass.set_bind_group(0, &bind_group, &[]);
            // draw fullscreen quad (triangle strip with 4 verts)
            rpass.draw(0..4, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
        Ok(())
    }
}
