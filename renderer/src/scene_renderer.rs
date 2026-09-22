//! Borrowed Scene backend. The UI never receives GPU cache handles or placement.
//!
//! This portable baseline uses dedicated cached buffers/textures and ordered
//! draws. It needs no bindless extensions, immediates or writable vertex storage.
//! Mask meshes are rasterized into six reusable viewport-sized R8 scratch images:
//! four cache a shared chain prefix; two ping-pong for arbitrarily deep tails.
//! Each ancestor multiplies the preceding coverage. Memory is O(viewport), not
//! O(mask_count * viewport). Conservative bounds limit clears and draws; all
//! preparation still occurs at the original phase even for culled objects.
//! This deliberately trades extra passes for bounded memory and arbitrary mesh
//! semantics; atlasing/batching remain backend optimizations, not ABI promises.
//!
//! A frame is one command buffer. Newly prepared cache entries are rolled back
//! if any callback fails, because commands recorded before that failure were not
//! submitted either. Existing cache entries survive. Eviction occurs only after
//! successful submission, never between phases (which would change snapshots).
//! Draw parameters occupy one aligned dynamic-uniform arena per frame. The
//! per-draw-buffer/per-pass prototype exhausted Vulkan resources at showcase
//! scale; an arena alone did not fix it. Batching and direct sampling of
//! coincident non-overlapping masks made that workload succeed (journal
//! 2026-09-23). The exact driver allocation responsible was not isolated.

use render_interface::*;
use std::collections::HashMap;
use wgpu::util::DeviceExt;

const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const COVERAGE: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

pub struct SceneTarget<'a> {
    /// Full, single-sample 2D attachment; size is taken from its texture.
    pub view: &'a wgpu::TextureView,
    pub viewport: [f32; 2],
    pub clear: wgpu::Color,
    /// Optional full-size sampled initial image, composited over clear before
    /// phase zero. It must not alias the destination or any source output.
    pub initial: Option<&'a wgpu::TextureView>,
}

#[derive(Debug, thiserror::Error)]
pub enum SceneError {
    #[error("invalid scene: {0}")]
    Invalid(String),
    #[error("resource {id} preparation failed: {source}")]
    Prepare { id: u64, source: PrepareError },
}
#[derive(Debug, Default, Clone, Copy)]
pub struct RenderStats {
    pub prepared: usize,
    pub cache_hits: usize,
    pub draw_calls: usize,
    /// Render passes containing draws (excludes initial/final clears).
    pub draw_batches: usize,
    pub mask_passes: usize,
    pub snapshot_copies: usize,
    pub cache_bytes: u64,
    pub evicted: usize,
}
struct Entry<T> {
    value: T,
    bytes: u64,
    last_used: u64,
    created_frame: u64,
}
struct Mesh {
    desc: MeshDescriptor,
    vertices: wgpu::Buffer,
    indices: Option<wgpu::Buffer>,
}
struct Image {
    desc: TextureDescriptor,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}
struct Surfaces {
    size: [u32; 2],
    color: Image,
    snapshot: Image,
    masks: [Image; 6],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    transform: Matrix4<f32>,
    viewport: [f32; 2],
    opacity: f32,
    masked: u32,
}

pub struct SceneRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    color_pipeline: wgpu::RenderPipeline,
    mask_pipeline: wgpu::RenderPipeline,
    clear_pipeline: wgpu::RenderPipeline,
    outputs: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    meshes: HashMap<MeshId, Entry<Mesh>>,
    textures: HashMap<TextureId, Entry<Image>>,
    masks: HashMap<MaskId, Entry<Image>>,
    surfaces: Option<Surfaces>,
    parameter_buffer: Option<wgpu::Buffer>,
    parameter_bytes: Vec<u8>,
    quad: Mesh,
    white: Image,
    frame: u64,
    budget: u64,
    stats: RenderStats,
}

impl SceneRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene resources"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(80),
                    },
                    count: None,
                },
                texture_binding(1, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture_binding(3, false),
                texture_binding(4, true),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene compositor"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene_renderer.wgsl").into()),
        });
        let color_pipeline = pipeline(
            device,
            &pipeline_layout,
            &shader,
            COLOR,
            "color",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let maximum = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Max,
        };
        let mask_pipeline = pipeline(
            device,
            &pipeline_layout,
            &shader,
            COVERAGE,
            "mask",
            Some(wgpu::BlendState {
                color: maximum,
                alpha: maximum,
            }),
        );
        let clear_pipeline = pipeline(device, &pipeline_layout, &shader, COVERAGE, "color", None);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let vertices = [
            Vertex {
                position: [0., 0., 0.],
                uv: [0., 0.],
            },
            Vertex {
                position: [0., 1., 0.],
                uv: [0., 1.],
            },
            Vertex {
                position: [1., 1., 0.],
                uv: [1., 1.],
            },
            Vertex {
                position: [0., 0., 0.],
                uv: [0., 0.],
            },
            Vertex {
                position: [1., 1., 0.],
                uv: [1., 1.],
            },
            Vertex {
                position: [1., 0., 0.],
                uv: [1., 0.],
            },
        ];
        let quad = Mesh {
            desc: MeshDescriptor::triangles(6, 0),
            vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("compositor quad"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            indices: None,
        };
        let white = make_image(device, TextureDescriptor::new([1, 1], COVERAGE));
        queue.write_texture(
            white.texture.as_image_copy(),
            &[255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            extent([1, 1]),
        );
        Self {
            device: device.clone(),
            queue: queue.clone(),
            layout,
            pipeline_layout,
            shader,
            sampler,
            color_pipeline,
            mask_pipeline,
            clear_pipeline,
            outputs: HashMap::new(),
            meshes: HashMap::new(),
            textures: HashMap::new(),
            masks: HashMap::new(),
            surfaces: None,
            parameter_buffer: None,
            parameter_bytes: Vec::new(),
            quad,
            white,
            frame: 0,
            budget: 128 * 1024 * 1024,
            stats: RenderStats::default(),
        }
    }
    /// Soft resident-resource budget. A single frame's working set is pinned;
    /// scratch attachments and command-buffer retention are not counted here.
    pub fn set_cache_budget(&mut self, bytes: u64) {
        self.budget = bytes;
    }
    pub fn stats(&self) -> RenderStats {
        self.stats
    }
    pub fn clear_cache(&mut self) {
        self.meshes.clear();
        self.textures.clear();
        self.masks.clear();
    }

    pub fn render(&mut self, scene: &Scene, target: SceneTarget<'_>) -> Result<(), SceneError> {
        self.validate(scene, &target)?;
        self.frame += 1;
        self.stats = RenderStats::default();
        let result = self.encode(scene, target);
        if result.is_err() {
            self.meshes.retain(|_, e| e.created_frame != self.frame);
            self.textures.retain(|_, e| e.created_frame != self.frame);
            self.masks.retain(|_, e| e.created_frame != self.frame);
        } else {
            self.evict(scene);
        }
        self.stats.cache_bytes = self.cache_bytes();
        result
    }

    fn encode(&mut self, scene: &Scene, target: SceneTarget<'_>) -> Result<(), SceneError> {
        let size = [
            target.view.texture().width(),
            target.view.texture().height(),
        ];
        if self.surfaces.as_ref().is_none_or(|s| s.size != size) {
            self.surfaces = Some(Surfaces {
                size,
                color: attachment(&self.device, size, COLOR),
                snapshot: attachment(&self.device, size, COLOR),
                masks: std::array::from_fn(|_| attachment(&self.device, size, COVERAGE)),
            });
        }
        // Take ownership locally to permit cache mutation while snapshots borrow
        // these attachments. Always put them back, including callback errors.
        let surfaces = self
            .surfaces
            .take()
            .expect("frame surfaces were just allocated");
        let result = self.encode_with_surfaces(scene, target, &surfaces);
        self.surfaces = Some(surfaces);
        result
    }
    fn encode_with_surfaces(
        &mut self,
        scene: &Scene,
        target: SceneTarget<'_>,
        s: &Surfaces,
    ) -> Result<(), SceneError> {
        let stride = u64::from(self.device.limits().min_uniform_buffer_offset_alignment)
            .max(80)
            .next_multiple_of(u64::from(
                self.device.limits().min_uniform_buffer_offset_alignment,
            ));
        let mut draw_capacity = 2usize;
        for object in scene.phases.iter().flat_map(|p| &p.objects) {
            draw_capacity += 1;
            let mut mask = object.mask;
            while let Some(i) = mask {
                draw_capacity += 2;
                mask = scene.pixel_masks[i.0 as usize].parent;
            }
        }
        let uniform_size = stride
            .checked_mul(draw_capacity as u64)
            .ok_or_else(|| SceneError::Invalid("draw parameter capacity overflow".into()))?;
        if uniform_size > self.device.limits().max_buffer_size || uniform_size > u64::from(u32::MAX)
        {
            return Err(SceneError::Invalid("too many draw parameters".into()));
        }
        if self
            .parameter_buffer
            .as_ref()
            .is_none_or(|buffer| buffer.size() < uniform_size)
        {
            self.parameter_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("reusable frame parameter arena"),
                size: uniform_size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let mut parameter_bytes = std::mem::take(&mut self.parameter_bytes);
        parameter_bytes.clear();
        let mut frame = DrawFrame {
            encoder: self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("scene frame"),
                }),
            uniforms: self
                .parameter_buffer
                .as_ref()
                .expect("arena was allocated")
                .clone(),
            bytes: parameter_bytes,
            stride: stride as usize,
            pending: Vec::new(),
            destination: None,
            batches: 0,
        };
        clear(&mut frame.encoder, &s.color.view, target.clear);
        let full = Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(
            target.viewport[0],
            target.viewport[1],
            1.,
        ));
        if let Some(initial) = target.initial {
            self.draw(
                &mut frame,
                &s.color.view,
                &self.color_pipeline,
                &self.quad,
                initial,
                &self.white.view,
                Params {
                    transform: full,
                    viewport: target.viewport,
                    opacity: 1.,
                    masked: 0,
                },
                None,
                None,
            );
        }
        let mut chain = Vec::new();
        let mut last_mask = None;
        let mut active_mask = 0;
        let mut prefix = Vec::new();
        for phase in &scene.phases {
            flush(&mut frame);
            frame.encoder.copy_texture_to_texture(
                s.color.texture.as_image_copy(),
                s.snapshot.texture.as_image_copy(),
                extent(s.size),
            );
            self.stats.snapshot_copies += 1;
            let snapshot = RenderSnapshot {
                color_texture: &s.snapshot.texture,
                color_view: &s.snapshot.view,
                size: s.size,
                format: COLOR,
            };
            // Prepare all resources before drawing any object of this phase.
            for object in &phase.objects {
                self.prepare_mesh(scene, object.mesh, &mut frame.encoder, snapshot)?;
                self.prepare_texture(scene, object.texture, &mut frame.encoder, snapshot)?;
                let mut mask = object.mask;
                while let Some(index) = mask {
                    let node = &scene.pixel_masks[index.0 as usize];
                    self.prepare_mesh(scene, node.mesh, &mut frame.encoder, snapshot)?;
                    self.prepare_mask(scene, node.texture, &mut frame.encoder, snapshot)?;
                    mask = node.parent;
                }
            }
            for object in &phase.objects {
                let direct = object.mask.and_then(|i| {
                    let m = &scene.pixel_masks[i.0 as usize];
                    (m.mesh == object.mesh
                        && m.transform == object.transform
                        && self.meshes[&m.mesh].value.desc.non_overlapping)
                        .then_some(m)
                });
                let screen_mask = direct.map_or(object.mask, |m| m.parent);
                let local_mask = direct.map(|m| &self.masks[&m.texture].value.view);
                let mut object_bounds = pixel_bounds(
                    self.meshes[&object.mesh].value.desc.bounds,
                    &object.transform,
                    target.viewport,
                    s.size,
                );
                chain.clear();
                let mut mask = screen_mask;
                while let Some(index) = mask {
                    chain.push(index);
                    mask = scene.pixel_masks[index.0 as usize].parent;
                }
                chain.reverse();
                for index in &chain {
                    let node = &scene.pixel_masks[index.0 as usize];
                    object_bounds = intersection(
                        object_bounds,
                        pixel_bounds(
                            self.meshes[&node.mesh].value.desc.bounds,
                            &node.transform,
                            target.viewport,
                            s.size,
                        ),
                    );
                }
                if object_bounds[2] == 0 || object_bounds[3] == 0 {
                    continue;
                }
                if screen_mask.is_some() && screen_mask != last_mask {
                    let common = prefix
                        .iter()
                        .zip(&chain)
                        .take_while(|(a, b)| a == b)
                        .count();
                    let mut bounds = [0, 0, s.size[0], s.size[1]];
                    for (depth, index) in chain.iter().enumerate() {
                        let node = &scene.pixel_masks[index.0 as usize];
                        bounds = intersection(
                            bounds,
                            pixel_bounds(
                                self.meshes[&node.mesh].value.desc.bounds,
                                &node.transform,
                                target.viewport,
                                s.size,
                            ),
                        );
                        active_mask = mask_slot(depth);
                        if depth < common {
                            continue;
                        }
                        // Clear only the conservative intersection. Pixels outside
                        // it are never sampled by a surviving object's scissor.
                        self.draw(
                            &mut frame,
                            &s.masks[active_mask].view,
                            &self.clear_pipeline,
                            &self.quad,
                            &self.white.view,
                            &self.white.view,
                            Params {
                                transform: full,
                                viewport: target.viewport,
                                opacity: 0.,
                                masked: 0,
                            },
                            Some(bounds),
                            None,
                        );
                        let parent = if depth == 0 {
                            &self.white.view
                        } else {
                            &s.masks[mask_slot(depth - 1)].view
                        };
                        self.draw(
                            &mut frame,
                            &s.masks[active_mask].view,
                            &self.mask_pipeline,
                            &self.meshes[&node.mesh].value,
                            &self.masks[&node.texture].value.view,
                            parent,
                            Params {
                                transform: node.transform,
                                viewport: target.viewport,
                                opacity: 1.,
                                masked: u32::from(depth != 0),
                            },
                            Some(bounds),
                            None,
                        );
                        self.stats.mask_passes += 1;
                    }
                    prefix.clear();
                    prefix.extend(chain.iter().take(4).copied());
                }
                last_mask = screen_mask;
                self.draw(
                    &mut frame,
                    &s.color.view,
                    &self.color_pipeline,
                    &self.meshes[&object.mesh].value,
                    &self.textures[&object.texture].value.view,
                    &s.masks[active_mask].view,
                    Params {
                        transform: object.transform,
                        viewport: target.viewport,
                        opacity: object.opacity,
                        masked: u32::from(screen_mask.is_some())
                            | if local_mask.is_some() { 2 } else { 0 },
                    },
                    Some(object_bounds),
                    local_mask,
                );
                self.stats.draw_calls += 1;
            }
        }
        let format = target.view.texture().format();
        let output = self
            .outputs
            .entry(format)
            .or_insert_with(|| {
                pipeline(
                    &self.device,
                    &self.pipeline_layout,
                    &self.shader,
                    format,
                    "color",
                    None,
                )
            })
            .clone();
        // No Load of uninitialized destination content: output covers the whole
        // attachment and replaces it; clear also makes empty scenes defined.
        flush(&mut frame);
        clear(&mut frame.encoder, target.view, wgpu::Color::TRANSPARENT);
        self.draw(
            &mut frame,
            target.view,
            &output,
            &self.quad,
            &s.color.view,
            &self.white.view,
            Params {
                transform: full,
                viewport: target.viewport,
                opacity: 1.,
                masked: 0,
            },
            None,
            None,
        );
        flush(&mut frame);
        self.stats.draw_batches = frame.batches;
        self.queue.write_buffer(&frame.uniforms, 0, &frame.bytes);
        self.queue.submit([frame.encoder.finish()]);
        self.parameter_bytes = frame.bytes;
        Ok(())
    }

    fn prepare_mesh(
        &mut self,
        scene: &Scene,
        id: MeshId,
        encoder: &mut wgpu::CommandEncoder,
        snapshot: RenderSnapshot<'_>,
    ) -> Result<(), SceneError> {
        if let Some(entry) = self.meshes.get_mut(&id) {
            entry.last_used = self.frame;
            self.stats.cache_hits += 1;
            return Ok(());
        }
        let source = scene.resources.mesh(id).expect("scene was validated");
        let desc = *source.descriptor();
        let vertices = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene vertices"),
            size: u64::from(desc.vertex_count) * 20,
            usage: desc.usages | wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let indices = (desc.index_count != 0).then(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("scene indices"),
                size: u64::from(desc.index_count) * 4,
                usage: desc.usages | wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        source
            .prepare(MeshPrepareContext {
                gpu: GpuPrepareContext {
                    device: &self.device,
                    encoder,
                    snapshot,
                },
                target: MeshTarget {
                    desc: &desc,
                    vertices: &vertices,
                    indices: indices.as_ref(),
                },
            })
            .map_err(|source| SceneError::Prepare {
                id: id.get(),
                source,
            })?;
        let bytes = u64::from(desc.vertex_count) * 20 + u64::from(desc.index_count) * 4;
        self.meshes.insert(
            id,
            Entry {
                value: Mesh {
                    desc,
                    vertices,
                    indices,
                },
                bytes,
                last_used: self.frame,
                created_frame: self.frame,
            },
        );
        self.stats.prepared += 1;
        Ok(())
    }
    fn prepare_texture(
        &mut self,
        scene: &Scene,
        id: TextureId,
        encoder: &mut wgpu::CommandEncoder,
        snapshot: RenderSnapshot<'_>,
    ) -> Result<(), SceneError> {
        if let Some(entry) = self.textures.get_mut(&id) {
            entry.last_used = self.frame;
            self.stats.cache_hits += 1;
            return Ok(());
        }
        let source = scene.resources.texture(id).expect("scene was validated");
        let image = make_image(&self.device, *source.descriptor());
        source
            .prepare(TexturePrepareContext {
                gpu: GpuPrepareContext {
                    device: &self.device,
                    encoder,
                    snapshot,
                },
                target: image.target(),
            })
            .map_err(|source| SceneError::Prepare {
                id: id.get(),
                source,
            })?;
        self.textures.insert(
            id,
            Entry {
                bytes: image.bytes(),
                value: image,
                last_used: self.frame,
                created_frame: self.frame,
            },
        );
        self.stats.prepared += 1;
        Ok(())
    }
    fn prepare_mask(
        &mut self,
        scene: &Scene,
        id: MaskId,
        encoder: &mut wgpu::CommandEncoder,
        snapshot: RenderSnapshot<'_>,
    ) -> Result<(), SceneError> {
        if let Some(entry) = self.masks.get_mut(&id) {
            entry.last_used = self.frame;
            self.stats.cache_hits += 1;
            return Ok(());
        }
        let source = scene.resources.mask(id).expect("scene was validated");
        let image = make_image(&self.device, *source.descriptor());
        source
            .prepare(MaskPrepareContext {
                gpu: GpuPrepareContext {
                    device: &self.device,
                    encoder,
                    snapshot,
                },
                target: image.target(),
            })
            .map_err(|source| SceneError::Prepare {
                id: id.get(),
                source,
            })?;
        self.masks.insert(
            id,
            Entry {
                bytes: image.bytes(),
                value: image,
                last_used: self.frame,
                created_frame: self.frame,
            },
        );
        self.stats.prepared += 1;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        frame: &mut DrawFrame,
        destination: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        mesh: &Mesh,
        image: &wgpu::TextureView,
        mask: &wgpu::TextureView,
        params: Params,
        scissor: Option<[u32; 4]>,
        local: Option<&wgpu::TextureView>,
    ) {
        let offset = frame.bytes.len();
        frame.bytes.extend_from_slice(bytemuck::bytes_of(&params));
        frame.bytes.resize(offset + frame.stride, 0);
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene draw"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &frame.uniforms,
                        offset: 0,
                        size: wgpu::BufferSize::new(80),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(image),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(mask),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(local.unwrap_or(&self.white.view)),
                },
            ],
        });
        if frame
            .destination
            .as_ref()
            .is_some_and(|view| view != destination)
        {
            flush(frame);
        }
        frame.destination = Some(destination.clone());
        frame.pending.push(DrawCall {
            pipeline: pipeline.clone(),
            group,
            vertices: mesh.vertices.clone(),
            indices: mesh.indices.clone(),
            desc: mesh.desc,
            offset: offset as u32,
            scissor,
        });
    }
    fn cache_bytes(&self) -> u64 {
        self.meshes.values().map(|e| e.bytes).sum::<u64>()
            + self.textures.values().map(|e| e.bytes).sum::<u64>()
            + self.masks.values().map(|e| e.bytes).sum::<u64>()
    }
    fn evict(&mut self, scene: &Scene) {
        let mut bytes = self.cache_bytes();
        // Pool presence is a tie-breaker retention hint, never a cache hit or
        // preparation request. Actual last use remains independently tracked.
        let mut candidates = Vec::new();
        for (id, e) in &self.meshes {
            if e.last_used != self.frame {
                candidates.push((
                    scene.resources.mesh(*id).is_some(),
                    e.last_used,
                    id.get(),
                    0,
                    e.bytes,
                ));
            }
        }
        for (id, e) in &self.textures {
            if e.last_used != self.frame {
                candidates.push((
                    scene.resources.texture(*id).is_some(),
                    e.last_used,
                    id.get(),
                    1,
                    e.bytes,
                ));
            }
        }
        for (id, e) in &self.masks {
            if e.last_used != self.frame {
                candidates.push((
                    scene.resources.mask(*id).is_some(),
                    e.last_used,
                    id.get(),
                    2,
                    e.bytes,
                ));
            }
        }
        candidates.sort_unstable();
        for (_, _, id, kind, size) in candidates {
            if bytes <= self.budget {
                break;
            }
            match kind {
                0 => self.meshes.retain(|k, _| k.get() != id),
                1 => self.textures.retain(|k, _| k.get() != id),
                _ => self.masks.retain(|k, _| k.get() != id),
            }
            bytes -= size;
            self.stats.evicted += 1;
        }
    }

    fn validate(&self, scene: &Scene, target: &SceneTarget<'_>) -> Result<(), SceneError> {
        let invalid = |msg: &str| SceneError::Invalid(msg.into());
        let destination = target.view.texture();
        if destination.dimension() != wgpu::TextureDimension::D2
            || destination.depth_or_array_layers() != 1
            || destination.mip_level_count() != 1
        {
            return Err(invalid(
                "destination must be a full single-layer, single-mip 2D view",
            ));
        }
        if !matches!(
            destination.format(),
            wgpu::TextureFormat::Rgba8Unorm
                | wgpu::TextureFormat::Rgba8UnormSrgb
                | wgpu::TextureFormat::Bgra8Unorm
                | wgpu::TextureFormat::Bgra8UnormSrgb
                | wgpu::TextureFormat::Rgba16Float
        ) {
            return Err(invalid("unsupported destination format"));
        }
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
                || texture.size() != destination.size()
                || texture.sample_count() != 1
                || texture.mip_level_count() != 1
                || texture.dimension() != wgpu::TextureDimension::D2
                || !texture
                    .usage()
                    .contains(wgpu::TextureUsages::TEXTURE_BINDING)
            {
                return Err(invalid(
                    "initial image must be a separate full-size sampled 2D image",
                ));
            }
            if !matches!(
                texture.format(),
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
        if target.viewport.iter().any(|v| !v.is_finite() || *v <= 0.) {
            return Err(invalid("viewport must be finite and positive"));
        }
        if target.view.texture().sample_count() != 1
            || !target
                .view
                .texture()
                .usage()
                .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
        {
            return Err(invalid(
                "destination must be a single-sample render attachment",
            ));
        }
        let check_mesh = |id| -> Result<(), SceneError> {
            let source = scene
                .resources
                .mesh(id)
                .ok_or_else(|| invalid("missing mesh definition"))?;
            let d = source.descriptor();
            if let Some([min, max]) = d.bounds {
                if (0..3).any(|i| !min[i].is_finite() || !max[i].is_finite() || min[i] > max[i]) {
                    return Err(invalid("invalid conservative mesh bounds"));
                }
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
            if u64::from(d.vertex_count) * 20 > self.device.limits().max_buffer_size
                || u64::from(d.index_count) * 4 > self.device.limits().max_buffer_size
            {
                return Err(invalid("mesh exceeds device buffer limit"));
            }
            if d.usages
                .intersects(wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::MAP_WRITE)
            {
                return Err(invalid("mesh outputs cannot be mappable"));
            }
            if self.meshes.get(&id).is_some_and(|e| e.value.desc != *d) {
                return Err(invalid("cached mesh ID changed descriptor"));
            }
            Ok(())
        };
        let check_image = |d: &TextureDescriptor| -> Result<(), SceneError> {
            if d.size
                .iter()
                .any(|v| *v == 0 || *v > self.device.limits().max_texture_dimension_2d)
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
            let usages =
                d.usages | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
            if !d
                .format
                .guaranteed_format_features(self.device.features())
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
            check_mesh(m.mesh)?;
            let d = scene
                .resources
                .mask(m.texture)
                .ok_or_else(|| invalid("missing mask definition"))?
                .descriptor();
            check_image(d)?;
            if self
                .masks
                .get(&m.texture)
                .is_some_and(|e| e.value.desc != *d)
            {
                return Err(invalid("cached mask ID changed descriptor"));
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
            check_mesh(o.mesh)?;
            let d = scene
                .resources
                .texture(o.texture)
                .ok_or_else(|| invalid("missing texture definition"))?
                .descriptor();
            check_image(d)?;
            if self
                .textures
                .get(&o.texture)
                .is_some_and(|e| e.value.desc != *d)
            {
                return Err(invalid("cached texture ID changed descriptor"));
            }
        }
        Ok(())
    }
}
impl Renderer for SceneRenderer {
    type Target<'a> = SceneTarget<'a>;
    type Error = SceneError;
    fn render(&mut self, scene: &Scene, target: SceneTarget<'_>) -> Result<(), SceneError> {
        SceneRenderer::render(self, scene, target)
    }
}
impl Image {
    fn target(&self) -> TextureTarget<'_> {
        TextureTarget {
            desc: &self.desc,
            texture: &self.texture,
            view: &self.view,
        }
    }
    fn bytes(&self) -> u64 {
        u64::from(self.desc.size[0])
            * u64::from(self.desc.size[1])
            * match self.desc.format {
                wgpu::TextureFormat::R8Unorm => 1,
                wgpu::TextureFormat::Rgba16Float => 8,
                _ => 4,
            }
    }
}
fn extent([width, height]: [u32; 2]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}
fn make_image(device: &wgpu::Device, desc: TextureDescriptor) -> Image {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene image"),
        size: extent(desc.size),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: desc.format,
        usage: desc.usages | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    Image {
        desc,
        texture,
        view,
    }
}
fn attachment(device: &wgpu::Device, size: [u32; 2], format: wgpu::TextureFormat) -> Image {
    make_image(
        device,
        TextureDescriptor {
            size,
            format,
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        },
    )
}
fn clear(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, color: wgpu::Color) {
    let attachments = [Some(wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(color),
            store: wgpu::StoreOp::Store,
        },
    })];
    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("scene clear"),
        color_attachments: &attachments,
        ..Default::default()
    });
}
fn texture_binding(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    fragment: &str,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("scene pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 20,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x2],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

struct DrawFrame {
    encoder: wgpu::CommandEncoder,
    uniforms: wgpu::Buffer,
    bytes: Vec<u8>,
    stride: usize,
    pending: Vec<DrawCall>,
    destination: Option<wgpu::TextureView>,
    batches: usize,
}
struct DrawCall {
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: Option<wgpu::Buffer>,
    desc: MeshDescriptor,
    offset: u32,
    scissor: Option<[u32; 4]>,
}
fn flush(frame: &mut DrawFrame) {
    let Some(destination) = frame.destination.take() else {
        return;
    };
    frame.batches += 1;
    let attachments = [Some(wgpu::RenderPassColorAttachment {
        view: &destination,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store: wgpu::StoreOp::Store,
        },
    })];
    {
        let mut pass = frame
            .encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene ordered batch"),
                color_attachments: &attachments,
                ..Default::default()
            });
        for draw in &frame.pending {
            let [x, y, w, h] = draw.scissor.unwrap_or([
                0,
                0,
                destination.texture().width(),
                destination.texture().height(),
            ]);
            pass.set_scissor_rect(x, y, w, h);
            pass.set_pipeline(&draw.pipeline);
            pass.set_bind_group(0, &draw.group, &[draw.offset]);
            pass.set_vertex_buffer(0, draw.vertices.slice(..));
            if let Some(indices) = &draw.indices {
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..draw.desc.index_count, 0, 0..1);
            } else {
                pass.draw(0..draw.desc.vertex_count, 0..1);
            }
        }
    }
    frame.pending.clear();
}
fn mask_slot(depth: usize) -> usize {
    if depth < 4 { depth } else { 4 + depth % 2 }
}
fn intersection(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    let x = a[0].max(b[0]);
    let y = a[1].max(b[1]);
    [
        x,
        y,
        (a[0] + a[2]).min(b[0] + b[2]).saturating_sub(x),
        (a[1] + a[3]).min(b[1] + b[3]).saturating_sub(y),
    ]
}
/// Homogeneous AABB projection. Crossing the camera plane makes corner bounds
/// unsafe, so retain the full viewport in that case. Bounds are optional.
fn pixel_bounds(
    bounds: Option<[[f32; 3]; 2]>,
    transform: &Matrix4<f32>,
    viewport: [f32; 2],
    size: [u32; 2],
) -> [u32; 4] {
    let full = [0, 0, size[0], size[1]];
    let Some([min, max]) = bounds else {
        return full;
    };
    let mut low = [f32::INFINITY; 2];
    let mut high = [f32::NEG_INFINITY; 2];
    for x in [min[0], max[0]] {
        for y in [min[1], max[1]] {
            for z in [min[2], max[2]] {
                let p = transform * nalgebra::Vector4::new(x, y, z, 1.);
                if p.w <= 0. || p.iter().any(|v| !v.is_finite()) {
                    return full;
                }
                for (i, v) in [p.x / p.w, p.y / p.w].into_iter().enumerate() {
                    let v = v * size[i] as f32 / viewport[i];
                    low[i] = low[i].min(v);
                    high[i] = high[i].max(v);
                }
            }
        }
    }
    let left = low[0].floor().clamp(0., size[0] as f32) as u32;
    let top = low[1].floor().clamp(0., size[1] as f32) as u32;
    let right = high[0].ceil().clamp(0., size[0] as f32) as u32;
    let bottom = high[1].ceil().clamp(0., size[1] as f32) as u32;
    [
        left,
        top,
        right.saturating_sub(left),
        bottom.saturating_sub(top),
    ]
}
