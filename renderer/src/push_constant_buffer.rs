use std::marker::PhantomData;

#[cfg(not(target_arch = "wasm32"))]
pub struct PcBuffer<T: bytemuck::Pod> {
    _phantom: PhantomData<T>,
}

#[cfg(target_arch = "wasm32")]
pub struct PcBuffer<T: bytemuck::Pod> {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    _phantom: PhantomData<T>,
}

#[cfg(not(target_arch = "wasm32"))]
impl<T: bytemuck::Pod> PcBuffer<T> {
    pub fn new(_device: &wgpu::Device, _bind_group_layout: &wgpu::BindGroupLayout) -> Self {
        Self {
            _phantom: PhantomData,
        }
    }

    pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("PcBuffer (native, empty)"),
            entries: &[],
        })
    }

    pub fn push_constant_ranges(
        shader_stages: wgpu::ShaderStages,
        size: u32,
    ) -> Vec<wgpu::PushConstantRange> {
        vec![wgpu::PushConstantRange {
            stages: shader_stages,
            range: 0..size,
        }]
    }

    pub fn apply_to_render_pass(
        &self,
        _queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        _group: u32,
        stages: wgpu::ShaderStages,
        offset: u32,
        data: &T,
    ) {
        pass.set_push_constants(stages, offset, bytemuck::bytes_of(data));
    }

    pub fn apply_to_compute_pass(
        &self,
        _queue: &wgpu::Queue,
        pass: &mut wgpu::ComputePass<'_>,
        _group: u32,
        data: &T,
    ) {
        pass.set_push_constants(0, bytemuck::bytes_of(data));
    }
}

#[cfg(target_arch = "wasm32")]
impl<T: bytemuck::Pod> PcBuffer<T> {
    pub fn new(device: &wgpu::Device, bind_group_layout: &wgpu::BindGroupLayout) -> Self {
        let size = std::mem::size_of::<T>() as u64;
        // Uniform buffers must be at least 16 bytes and aligned to 16 bytes.
        let aligned_size = ((size.max(16) + 15) / 16) * 16;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PcBuffer uniform"),
            size: aligned_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("PcBuffer bind group"),
            layout: bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            buffer,
            bind_group,
            _phantom: PhantomData,
        }
    }

    pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("PcBuffer uniform layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        })
    }

    pub fn push_constant_ranges(
        _shader_stages: wgpu::ShaderStages,
        _size: u32,
    ) -> Vec<wgpu::PushConstantRange> {
        vec![]
    }

    pub fn apply_to_render_pass(
        &self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        group: u32,
        _stages: wgpu::ShaderStages,
        _offset: u32,
        data: &T,
    ) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(data));
        pass.set_bind_group(group, &self.bind_group, &[]);
    }

    pub fn apply_to_compute_pass(
        &self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::ComputePass<'_>,
        group: u32,
        data: &T,
    ) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(data));
        pass.set_bind_group(group, &self.bind_group, &[]);
    }
}
