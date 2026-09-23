//! Framework-owned scene construction. Widgets receive only Draw, never phases.
//! Draw records are emitted directly into reusable frame storage. A backdrop draw
//! samples everything painted before it; the framework creates the phase boundary.
//! Resource definitions and GPU content retain their IDs independently of Objects.
use render_interface::*;
use std::{collections::HashSet, sync::LazyLock};

pub fn unit_quad() -> MeshSource {
    quad_source().clone()
}
fn quad_source() -> &'static MeshSource {
    static QUAD: LazyLock<MeshSource> = LazyLock::new(|| {
        let mut desc = MeshDescriptor::triangles(6, 0);
        desc.bounds = Some([[0., 0., 0.], [1., 1., 0.]]);
        desc.non_overlapping = true;
        MeshSource::new(desc, |mut c| {
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
                &mut c.gpu,
                c.target.vertices,
                bytemuck::cast_slice(&vertices),
            )
        })
    });
    &QUAD
}

#[derive(Default)]
/// Reusable framework storage. Call begin, invoke draw writers in paint order,
/// then finish before handing scene to a renderer. Finish prunes on errors too.
pub struct Frame {
    pub scene: Scene,
    meshes: HashSet<MeshId>,
    textures: HashSet<TextureId>,
    masks: HashSet<MaskId>,
    phase: usize,
    error: Option<String>,
}
impl Frame {
    pub fn begin(&mut self) {
        for p in &mut self.scene.phases {
            p.objects.clear();
        }
        if self.scene.phases.is_empty() {
            self.scene.phases.push(Phase::default());
        }
        self.scene.pixel_masks.clear();
        self.meshes.clear();
        self.textures.clear();
        self.masks.clear();
        self.phase = 0;
        self.error = None;
    }
    pub fn draw(
        &mut self,
        transform: Matrix4<f32>,
        mask: Option<PixelMaskIndex>,
        opacity: f32,
    ) -> Draw<'_> {
        Draw {
            frame: self,
            transform,
            mask,
            opacity,
        }
    }
    pub fn finish(&mut self) -> Result<(), String> {
        self.scene
            .resources
            .retain_meshes(|id| self.meshes.contains(&id));
        self.scene
            .resources
            .retain_textures(|id| self.textures.contains(&id));
        self.scene
            .resources
            .retain_masks(|id| self.masks.contains(&id));
        self.scene.phases.truncate(self.phase + 1);
        self.error.take().map_or(Ok(()), Err)
    }
}

/// A borrowed writer into the framework's final frame; no local Scene or phase API.
/// Coordinates are widget-local. Scopes compose transforms and inherit clip/opacity.
pub struct Draw<'a> {
    frame: &'a mut Frame,
    transform: Matrix4<f32>,
    mask: Option<PixelMaskIndex>,
    opacity: f32,
}
impl Draw<'_> {
    /// Resolved local-to-viewport placement, including nested translated scopes.
    /// Backdrop generators capture this when mapping local pixels to the snapshot.
    pub fn transform(&self) -> Matrix4<f32> {
        self.transform
    }
    pub fn mesh(&mut self, source: &MeshSource) -> MeshId {
        self.frame.meshes.insert(source.id());
        if let Err(e) = self.frame.scene.resources.share_mesh(source) {
            self.frame.error = Some(e.to_string());
        }
        source.id()
    }
    pub fn texture(&mut self, source: &TextureSource) -> TextureId {
        self.frame.textures.insert(source.id());
        if let Err(e) = self.frame.scene.resources.share_texture(source) {
            self.frame.error = Some(e.to_string());
        }
        source.id()
    }
    pub fn mask_source(&mut self, source: &MaskSource) -> MaskId {
        self.frame.masks.insert(source.id());
        if let Err(e) = self.frame.scene.resources.share_mask(source) {
            self.frame.error = Some(e.to_string());
        }
        source.id()
    }
    /// Emit an ordinary object. Mask indices, if supplied, come from this frame.
    pub fn object(&mut self, mut object: Object) {
        let mut mask = object.mask.or(self.mask);
        while let Some(index) = mask {
            let Some(node) = self.frame.scene.pixel_masks.get(index.0 as usize) else {
                self.frame.error = Some("invalid draw mask".into());
                return;
            };
            if node.parent.is_some_and(|parent| parent.0 >= index.0)
                || self.frame.scene.resources.mesh(node.mesh).is_none()
                || self.frame.scene.resources.mask(node.texture).is_none()
            {
                self.frame.error = Some("invalid draw mask definition/parent".into());
                return;
            }
            self.frame.meshes.insert(node.mesh);
            self.frame.masks.insert(node.texture);
            mask = node.parent;
        }
        if self.frame.scene.resources.mesh(object.mesh).is_none()
            || self.frame.scene.resources.texture(object.texture).is_none()
        {
            self.frame.error = Some("missing draw resource".into());
            return;
        }
        self.frame.meshes.insert(object.mesh);
        self.frame.textures.insert(object.texture);
        object.transform = self.transform * object.transform;
        object.mask = object.mask.or(self.mask);
        object.opacity *= self.opacity;
        self.frame.scene.phases[self.frame.phase]
            .objects
            .push(object);
    }
    /// Paint a resource that reads the background at this paint position.
    /// Earlier widgets and earlier draws of this widget are included. No later
    /// draws are included. A changed background still requires a fresh content ID.
    pub fn backdrop(&mut self, object: Object) {
        if !self.frame.scene.phases[self.frame.phase].objects.is_empty() {
            self.frame.phase += 1;
            if self.frame.phase == self.frame.scene.phases.len() {
                self.frame.scene.phases.push(Phase::default());
            }
        }
        self.object(object);
    }
    /// Compose local placement without storing a child drawing tree.
    pub fn translated(&mut self, transform: Matrix4<f32>, paint: impl FnOnce(&mut Draw<'_>)) {
        let mut child = Draw {
            frame: self.frame,
            transform: self.transform * transform,
            mask: self.mask,
            opacity: self.opacity,
        };
        paint(&mut child);
    }
    /// Scope an arbitrary mesh/coverage mask. Only coverage inherits; geometry
    /// stays in local coordinates and is resolved once when the mask is emitted.
    pub fn masked(
        &mut self,
        mesh: &MeshSource,
        coverage: &MaskSource,
        transform: Matrix4<f32>,
        paint: impl FnOnce(&mut Draw<'_>),
    ) {
        let mesh = self.mesh(mesh);
        let texture = self.mask_source(coverage);
        let Ok(index) = u32::try_from(self.frame.scene.pixel_masks.len()) else {
            self.frame.error = Some("mask index capacity exceeded".into());
            return;
        };
        let index = PixelMaskIndex(index);
        self.frame.scene.pixel_masks.push(PixelMask {
            mesh,
            texture,
            transform: self.transform * transform,
            parent: self.mask,
        });
        let mut child = Draw {
            frame: self.frame,
            transform: self.transform,
            mask: Some(index),
            opacity: self.opacity,
        };
        paint(&mut child);
    }
    pub fn quad(
        &mut self,
        texture: &TextureSource,
        size: [f32; 2],
        transform: Matrix4<f32>,
        mask: Option<&MaskSource>,
    ) {
        let mesh = self.mesh(quad_source());
        let texture = self.texture(texture);
        let transform = transform
            * Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(size[0], size[1], 1.));
        let mut object = Object::new(mesh, texture, transform);
        if let Some(source) = mask {
            let texture = self.mask_source(source);
            let Ok(index) = u32::try_from(self.frame.scene.pixel_masks.len()) else {
                self.frame.error = Some("mask index capacity exceeded".into());
                return;
            };
            let index = PixelMaskIndex(index);
            self.frame.scene.pixel_masks.push(PixelMask {
                mesh,
                texture,
                transform: self.transform * transform,
                parent: self.mask,
            });
            object.mask = Some(index);
        }
        self.object(object);
    }
}
pub fn push_quad(
    draw: &mut Draw<'_>,
    texture: &TextureSource,
    size: [f32; 2],
    transform: Matrix4<f32>,
    mask: Option<&MaskSource>,
) {
    draw.quad(texture, size, transform, mask);
}
