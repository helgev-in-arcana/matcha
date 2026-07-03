use bevy_ecs::resource::Resource;

#[derive(Resource)]
pub struct GpuResource {
    pub gpu: gpu_utils::gpu::Gpu,
}
