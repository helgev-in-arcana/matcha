//! CPU assembly of a complete [`Scene`] in framework paint order.
//!
//! A framework owns [`Frame`], calls [`Frame::begin`], invokes its producers with
//! borrowed [`Draw`] writers, and calls [`Frame::finish`] before rendering. A writer
//! emits Objects directly; it stores no child tree, local Scene or phase numbers.
//! [`Draw::backdrop`] starts a phase at its paint position so newly referenced
//! sources see all preceding paint, including earlier producers. Sources retain
//! the interface's immutable-content and first-reference rules: reusing an ID does
//! not request regeneration or move its preparation to a later phase.
//!
//! Producers share immutable source definitions and register them through Draw.
//! Registration also acts as a retention hint, even without a drawing reference.
//! `finish` removes definitions neither registered nor referenced this frame. It
//! does so on failure too; the first assembly error remains until the next `begin`.
//! A failed frame must not be rendered. Assembly never executes a source callback
//! or allocates GPU resources; renderer validation and GPU errors remain separate.
//!
//! Scope transforms compose once, while mask coverage inherits through absolute
//! mask transforms. An explicit Object mask is a complete frame-local chain and
//! must contain the writer's inherited mask, so it cannot bypass an outer clip.
//! Prefer [`Draw::masked`] or [`Draw::quad`] to construct such chains safely.
//!
//! ```
//! use render_interface::{Matrix4, TextureSource};
//! use scene_builder::Frame;
//!
//! fn frame_for(texture: &TextureSource) -> Result<Frame, String> {
//!     let mut frame = Frame::default();
//!     frame.begin();
//!     frame.draw(Matrix4::identity(), None, 1.0).quad(
//!         texture, [80.0, 40.0], Matrix4::identity(), None,
//!     );
//!     frame.finish()?;
//!     Ok(frame) // The renderer can now borrow frame.scene.
//! }
//! ```
use render_interface::*;
use std::{collections::HashSet, sync::LazyLock};

/// Shared immutable mesh definition for a `[0, 1]` quad with matching UVs.
/// Cloning this definition preserves its ID and shares its generator allocation.
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

/// Reusable framework storage. Call begin, invoke draw writers in paint order,
/// then finish before handing scene to a renderer.
///
/// Public Scene access lets the framework install inherited masks. It must
/// register or reference their resource definitions during the frame. Draw checks
/// referenced mask chains; the renderer validates the remaining Scene contract.
#[derive(Default)]
pub struct Frame {
    pub scene: Scene,
    meshes: HashSet<MeshId>,
    textures: HashSet<TextureId>,
    masks: HashSet<MaskId>,
    phase: usize,
    error: Option<String>,
}
impl Frame {
    /// Begin a fresh frame, clearing draw records and the previous error while
    /// retaining definitions until `finish` decides which ones remain in use.
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
    /// Borrow a producer writer after `begin`. The inherited mask index belongs
    /// to this frame's Scene; its transform is already absolute. Object placement
    /// and opacity are multiplied by these scope values when emitted.
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
    /// Prune definitions absent from this frame and report its first assembly
    /// error. Calling this again returns the same result until `begin` resets the
    /// frame (or later writing introduces an error into a previously valid one).
    /// An error does not roll back emitted Objects; do not render that frame.
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
        self.error.clone().map_or(Ok(()), Err)
    }

    fn fail(&mut self, error: impl ToString) {
        if self.error.is_none() {
            self.error = Some(error.to_string());
        }
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
    /// Register or retain a mesh definition without executing its generator.
    pub fn mesh(&mut self, source: &MeshSource) -> MeshId {
        self.frame.meshes.insert(source.id());
        if let Err(e) = self.frame.scene.resources.share_mesh(source) {
            self.frame.fail(e);
        }
        source.id()
    }
    /// Register or retain a colour definition without executing its generator.
    pub fn texture(&mut self, source: &TextureSource) -> TextureId {
        self.frame.textures.insert(source.id());
        if let Err(e) = self.frame.scene.resources.share_texture(source) {
            self.frame.fail(e);
        }
        source.id()
    }
    /// Register or retain a coverage definition without executing its generator.
    pub fn mask_source(&mut self, source: &MaskSource) -> MaskId {
        self.frame.masks.insert(source.id());
        if let Err(e) = self.frame.scene.resources.share_mask(source) {
            self.frame.fail(e);
        }
        source.id()
    }
    /// Emit an ordinary object using local geometry and scope opacity.
    ///
    /// `None` inherits this writer's mask. An explicit mask index must belong to
    /// this frame and its ancestor chain must contain the inherited mask, if any.
    /// Unrelated chains are rejected instead of silently replacing an outer clip.
    /// Register definitions first; errors are reported by `Frame::finish`.
    pub fn object(&mut self, mut object: Object) {
        let resolved_mask = object.mask.or(self.mask);
        let mut mask = resolved_mask;
        let mut includes_inherited_mask = self.mask.is_none();
        while let Some(index) = mask {
            includes_inherited_mask |= Some(index) == self.mask;
            let Some(node) = self.frame.scene.pixel_masks.get(index.0 as usize) else {
                self.frame.fail("invalid draw mask");
                return;
            };
            if node.parent.is_some_and(|parent| parent.0 >= index.0)
                || self.frame.scene.resources.mesh(node.mesh).is_none()
                || self.frame.scene.resources.mask(node.texture).is_none()
            {
                self.frame.fail("invalid draw mask definition/parent");
                return;
            }
            self.frame.meshes.insert(node.mesh);
            self.frame.masks.insert(node.texture);
            mask = node.parent;
        }
        if !includes_inherited_mask {
            self.frame
                .fail("explicit draw mask must include the inherited mask");
            return;
        }
        if self.frame.scene.resources.mesh(object.mesh).is_none()
            || self.frame.scene.resources.texture(object.texture).is_none()
        {
            self.frame.fail("missing draw resource");
            return;
        }
        self.frame.meshes.insert(object.mesh);
        self.frame.textures.insert(object.texture);
        object.transform = self.transform * object.transform;
        object.mask = resolved_mask;
        object.opacity *= self.opacity;
        self.frame.scene.phases[self.frame.phase]
            .objects
            .push(object);
    }
    /// Paint a resource that reads the background at this paint position.
    /// Newly referenced sources see earlier writers and earlier draws of this
    /// writer, never later draws. Already referenced or resident IDs keep their
    /// immutable content; a changed background requires a fresh content ID.
    /// At the beginning of an empty phase this does not add an empty phase.
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
            self.frame.fail("mask index capacity exceeded");
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
    /// Emit a texture-scaled unit quad. Optional local coverage is intersected
    /// with the inherited mask and shares the Object's resolved transform.
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
                self.frame.fail("mask index capacity exceeded");
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
/// Free-function equivalent of [`Draw::quad`].
pub fn push_quad(
    draw: &mut Draw<'_>,
    texture: &TextureSource,
    size: [f32; 2],
    transform: Matrix4<f32>,
    mask: Option<&MaskSource>,
) {
    draw.quad(texture, size, transform, mask);
}

#[cfg(test)]
mod tests;
