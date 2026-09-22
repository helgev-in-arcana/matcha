//! CPU-side paint assembly. No GPU allocation or backend dependency.
//!
//! Widgets cache immutable bitmaps and local paint trees. [`SceneBuilder`]
//! resolves their transforms, clips and paint order into the flat upstream
//! contract. Its persistent resource pool retains unchanged source closures;
//! only new visible content creates a source. Bitmap storage may be shared by
//! UI caches, but ResourcePool owns Source values directly. Dropping GPU caches
//! therefore never invalidates a widget or forces text to be shaped again.

use nalgebra::{Matrix4, Vector3};
pub use render_interface;
use render_interface::*;
use std::{collections::HashSet, sync::Arc};

#[derive(Debug)]
struct BitmapData {
    size: [u32; 2],
    bytes: Vec<u8>,
    texture: TextureId,
    mask: MaskId,
    coverage: bool,
}
#[derive(Debug, Clone)]
pub struct Bitmap(Arc<BitmapData>);
impl Bitmap {
    /// Premultiplied, sRGB-encoded colour bytes.
    pub fn rgba(size: [u32; 2], bytes: Vec<u8>) -> Result<Self, &'static str> {
        Self::new(size, bytes, false)
    }
    pub fn coverage(size: [u32; 2], bytes: Vec<u8>) -> Result<Self, &'static str> {
        Self::new(size, bytes, true)
    }
    fn new(size: [u32; 2], bytes: Vec<u8>, coverage: bool) -> Result<Self, &'static str> {
        let count = u64::from(size[0]) * u64::from(size[1]) * (if coverage { 1 } else { 4 });
        if size.contains(&0) || count != bytes.len() as u64 {
            return Err("bitmap dimensions do not match bytes");
        }
        Ok(Self(Arc::new(BitmapData {
            size,
            bytes,
            texture: TextureId::new(),
            mask: MaskId::new(),
            coverage,
        })))
    }
    pub fn size(&self) -> [u32; 2] {
        self.0.size
    }
    pub fn texture_id(&self) -> TextureId {
        self.0.texture
    }
    pub fn mask_id(&self) -> MaskId {
        self.0.mask
    }
    fn desc(&self) -> TextureDescriptor {
        TextureDescriptor::new(
            self.0.size,
            if self.0.coverage {
                wgpu::TextureFormat::R8Unorm
            } else {
                wgpu::TextureFormat::Rgba8UnormSrgb
            },
        )
    }
    pub fn register_texture(&self, pool: &mut ResourcePool) -> TextureId {
        let id = self.texture_id();
        if pool.texture(id).is_none() {
            let data = self.clone();
            pool.insert_texture(TextureSource::with_id(id, self.desc(), move |mut ctx| {
                upload_texture(&mut ctx.gpu, &ctx.target, &data.0.bytes)
            }))
            .expect("ID absence checked in exclusively borrowed pool");
        }
        id
    }
    pub fn register_mask(&self, pool: &mut ResourcePool) -> MaskId {
        let id = self.mask_id();
        if pool.mask(id).is_none() {
            let data = self.clone();
            pool.insert_mask(MaskSource::with_id(id, self.desc(), move |mut ctx| {
                upload_texture(&mut ctx.gpu, &ctx.target, &data.0.bytes)
            }))
            .expect("ID absence checked in exclusively borrowed pool");
        }
        id
    }
}

/// UI-local grouping only. The renderer never receives this tree.
#[derive(Clone, Default, Debug)]
pub struct RenderNode {
    texture: Option<(Bitmap, Matrix4<f32>)>,
    stencil: Option<(Bitmap, Matrix4<f32>)>,
    children: Vec<(Arc<RenderNode>, Matrix4<f32>)>,
    phase: usize,
    custom: Option<CustomPaint>,
}
/// Resolved UI placement for a custom Scene contributor. Popup contributors
/// may deliberately choose a different mask/phase; picking remains UI policy.
#[derive(Clone, Copy)]
pub struct PaintPlacement {
    pub transform: Matrix4<f32>,
    pub mask: Option<PixelMaskIndex>,
    pub opacity: f32,
    pub phase: usize,
}
type PaintCallback = dyn Fn(&mut Scene, PaintPlacement) + Send + Sync;
#[derive(Clone)]
struct CustomPaint(Arc<PaintCallback>);
impl std::fmt::Debug for CustomPaint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CustomPaint")
    }
}
impl RenderNode {
    pub fn new() -> Self {
        Self::default()
    }
    /// Contribute arbitrary meshes, GPU sources and phase effects through a
    /// normal RenderItem. Called during each frame's CPU Scene assembly, even
    /// when this paint node is cached. Reuse source IDs for unchanged content;
    /// use new IDs whenever captured background/animation inputs change.
    pub fn custom(paint: impl Fn(&mut Scene, PaintPlacement) + Send + Sync + 'static) -> Self {
        Self {
            custom: Some(CustomPaint(Arc::new(paint))),
            ..Self::default()
        }
    }
    pub fn with_texture(mut self, texture: Bitmap, size: [f32; 2], position: Matrix4<f32>) -> Self {
        self.texture = Some((texture, position * scale(size)));
        self
    }
    /// Self-only coverage. Subtree clipping comes from the extracted clip arena.
    pub fn with_stencil(mut self, stencil: Bitmap, size: [f32; 2], position: Matrix4<f32>) -> Self {
        self.stencil = Some((stencil, position * scale(size)));
        self
    }
    /// Absolute compositing phase; does not change picking or UI ancestry.
    pub fn in_phase(mut self, phase: usize) -> Self {
        self.phase = phase;
        self
    }
    pub fn push_child(&mut self, child: impl Into<Arc<RenderNode>>, transform: Matrix4<f32>) {
        self.children.push((child.into(), transform));
    }
    pub fn add_child(mut self, child: impl Into<Arc<RenderNode>>, transform: Matrix4<f32>) -> Self {
        self.push_child(child, transform);
        self
    }
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(|(n, _)| n.count()).sum::<usize>()
    }
}
fn scale(size: [f32; 2]) -> Matrix4<f32> {
    Matrix4::new_nonuniform_scaling(&Vector3::new(size[0], size[1], 1.))
}

/// Reusable UI-owned Scene. `begin`/`push`/`finish` are assembly helpers; the
/// renderer sees only the completed, self-contained `scene()` borrow.
pub struct SceneBuilder {
    scene: Scene,
    quad: MeshId,
    white: Bitmap,
}
impl Default for SceneBuilder {
    fn default() -> Self {
        Self::new()
    }
}
impl SceneBuilder {
    pub fn new() -> Self {
        let mut scene = Scene::default();
        let quad = scene
            .resources
            .insert_mesh(unit_quad())
            .expect("new pool is empty");
        Self {
            scene,
            quad,
            white: Bitmap::coverage([1, 1], vec![255]).expect("one coverage texel"),
        }
    }
    pub fn scene(&self) -> &Scene {
        &self.scene
    }
    /// Extension point for custom mesh/GPU generators and post-processing.
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }
    pub fn begin(&mut self) {
        for phase in &mut self.scene.phases {
            phase.objects.clear();
        }
        self.scene.pixel_masks.clear();
    }
    pub fn push_clip(&mut self, parent: Option<u32>, transform: Matrix4<f32>) -> PixelMaskIndex {
        let texture = self.white.register_mask(&mut self.scene.resources);
        let index = PixelMaskIndex(
            self.scene
                .pixel_masks
                .len()
                .try_into()
                .expect("mask index capacity exhausted"),
        );
        self.scene.pixel_masks.push(PixelMask {
            mesh: self.quad,
            texture,
            transform,
            parent: parent.map(PixelMaskIndex),
        });
        index
    }
    pub fn push(
        &mut self,
        node: &RenderNode,
        transform: Matrix4<f32>,
        clip: Option<PixelMaskIndex>,
        opacity: f32,
    ) {
        if let Some(paint) = &node.custom {
            (paint.0)(
                &mut self.scene,
                PaintPlacement {
                    transform,
                    mask: clip,
                    opacity,
                    phase: node.phase,
                },
            );
        }
        if let Some((bitmap, position)) = &node.texture {
            let texture = bitmap.register_texture(&mut self.scene.resources);
            let mut object = Object::new(self.quad, texture, transform * position);
            object.opacity = opacity;
            object.mask = clip;
            if let Some((bitmap, position)) = &node.stencil {
                let texture = bitmap.register_mask(&mut self.scene.resources);
                let index = PixelMaskIndex(
                    self.scene
                        .pixel_masks
                        .len()
                        .try_into()
                        .expect("mask index capacity exhausted"),
                );
                self.scene.pixel_masks.push(PixelMask {
                    mesh: self.quad,
                    texture,
                    transform: transform * position,
                    parent: clip,
                });
                object.mask = Some(index);
            }
            self.scene
                .phases
                .resize_with(self.scene.phases.len().max(node.phase + 1), Phase::default);
            self.scene.phases[node.phase].objects.push(object);
        }
        for (child, local) in &node.children {
            self.push(child, transform * local, clip, opacity);
        }
    }
    /// Remove CPU definitions no longer referenced by this assembled scene.
    /// GPU eviction is a separate renderer policy. Custom callers needing
    /// unreferenced retention hints may omit this assembly convenience.
    pub fn finish(&mut self) {
        let mut meshes = HashSet::from([self.quad]);
        let mut textures = HashSet::new();
        let mut masks = HashSet::new();
        for o in self.scene.phases.iter().flat_map(|p| &p.objects) {
            meshes.insert(o.mesh);
            textures.insert(o.texture);
        }
        for m in &self.scene.pixel_masks {
            meshes.insert(m.mesh);
            masks.insert(m.texture);
        }
        self.scene
            .resources
            .retain_meshes(|id| meshes.contains(&id));
        self.scene
            .resources
            .retain_textures(|id| textures.contains(&id));
        self.scene.resources.retain_masks(|id| masks.contains(&id));
    }
}
pub fn unit_quad() -> MeshSource {
    let mut desc = MeshDescriptor::triangles(6, 0);
    desc.bounds = Some([[0., 0., 0.], [1., 1., 0.]]);
    desc.non_overlapping = true;
    MeshSource::new(desc, |mut ctx| {
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
        upload_buffer(
            &mut ctx.gpu,
            ctx.target.vertices,
            bytemuck::cast_slice(&vertices),
        )
    })
}
