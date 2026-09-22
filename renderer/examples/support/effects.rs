//! GPU-generator examples using only the public upstream contract.
use render_interface::*;

/// A compute-generated unit triangle; proves mesh outputs can be storage buffers.
pub fn compute_triangle() -> MeshSource {
    let mut desc = MeshDescriptor::triangles(3, 0);
    desc.usages = wgpu::BufferUsages::STORAGE;
    MeshSource::new(desc, |ctx| {
        let shader = ctx
            .gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("compute mesh"),
                source: wgpu::ShaderSource::Wgsl(
                    r#"
@group(0) @binding(0) var<storage,read_write> vertices: array<f32>;
@compute @workgroup_size(1) fn main() {
    let values=array<f32,15>(0.,0.,0.,0.,0., 1.,0.,0.,1.,0., 0.,1.,0.,0.,1.);
    for(var i=0u;i<15u;i++){vertices[i]=values[i];}
}
"#
                    .into(),
                ),
            });
        let pipeline = ctx
            .gpu
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("mesh generation"),
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
        let group = ctx
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mesh target"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ctx.target.vertices.as_entire_binding(),
                }],
            });
        let mut pass = ctx.gpu.encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
        Ok(())
    })
}

/// Mode 0: 9x9 box blur; 1: wave refraction; 2: Mandelbrot coverage;
/// 3: whole-image invert (deferred-style final processing).
pub fn compute_image(
    gpu: &mut GpuPrepareContext<'_>,
    target: &TextureTarget<'_>,
    mode: u32,
) -> PrepareResult {
    let code = format!(
        r#"
@group(0) @binding(0) var input_image: texture_2d<f32>;
@group(0) @binding(1) var output_image: texture_storage_2d<rgba8unorm,write>;
fn sample_at(p:vec2<i32>) -> vec4<f32> {{
    return textureLoad(input_image,clamp(p,vec2<i32>(0),vec2<i32>(textureDimensions(input_image))-vec2<i32>(1)),0);
}}
@compute @workgroup_size(8,8) fn main(@builtin(global_invocation_id) id:vec3<u32>) {{
    let size=textureDimensions(output_image);
    if any(id.xy>=size) {{return;}}
    let uv=(vec2<f32>(id.xy)+0.5)/vec2<f32>(size);
    let p=vec2<i32>(uv*vec2<f32>(textureDimensions(input_image)));
    var result=vec4<f32>(0.);
    if {mode}u==0u {{
        for(var y=-4;y<=4;y++) {{for(var x=-4;x<=4;x++) {{result+=sample_at(p+vec2<i32>(x,y))/81.;}}}}
    }} else if {mode}u==1u {{
        result=sample_at(p+vec2<i32>(i32(8.*sin(uv.y*32.)),0));
    }} else if {mode}u==2u {{
        let c=vec2<f32>(uv.x*3.2-2.2,uv.y*2.4-1.2);
        var z=vec2<f32>(0.);var n=0u;
        for(var i=0u;i<48u;i++) {{
            z=vec2<f32>(z.x*z.x-z.y*z.y,2.*z.x*z.y)+c;
            if dot(z,z)>4. {{break;}} n++;
        }}
        let coverage=select(f32(n)/48.,1.,n==48u);
        result=vec4<f32>(coverage,coverage,coverage,1.);
    }} else {{let c=sample_at(p);result=vec4<f32>(vec3<f32>(c.a)-c.rgb,c.a);}}
    textureStore(output_image,vec2<i32>(id.xy),result);
}}
"#
    );
    let shader = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene effect generator"),
            source: wgpu::ShaderSource::Wgsl(code.into()),
        });
    let pipeline = gpu
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("scene effect"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("effect targets"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(gpu.snapshot.color_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(target.view),
            },
        ],
    });
    let mut pass = gpu.encoder.begin_compute_pass(&Default::default());
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.dispatch_workgroups(
        target.desc.size[0].div_ceil(8),
        target.desc.size[1].div_ceil(8),
        1,
    );
    Ok(())
}
pub fn effect(size: [u32; 2], mode: u32) -> TextureSource {
    let mut desc = TextureDescriptor::new(size, wgpu::TextureFormat::Rgba8Unorm);
    desc.usages = wgpu::TextureUsages::STORAGE_BINDING;
    TextureSource::new(desc, move |mut c| {
        compute_image(&mut c.gpu, &c.target, mode)
    })
}
pub fn fractal(size: [u32; 2]) -> MaskSource {
    let mut desc = TextureDescriptor::new(size, wgpu::TextureFormat::Rgba8Unorm);
    desc.usages = wgpu::TextureUsages::STORAGE_BINDING;
    MaskSource::new(desc, move |mut c| compute_image(&mut c.gpu, &c.target, 2))
}
