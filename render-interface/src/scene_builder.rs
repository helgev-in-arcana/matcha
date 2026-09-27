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
//! Enable the `scene-builder` feature to use these helpers from the crate root.
//!
//! ```
//! use render_interface::{Frame, Matrix4, TextureSource};
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
use crate::interface::*;
use std::{collections::HashSet, sync::LazyLock};

/// Shared immutable mesh definition for a `[0, 1]` quad with matching UVs.
/// Cloning this definition preserves its ID and shares its generator allocation.
pub fn unit_quad() -> MeshSource {
    QUAD.clone()
}

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
        let mesh = self.mesh(&QUAD);
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
mod tests {
    use super::*;

    fn mesh() -> MeshSource {
        MeshSource::new(MeshDescriptor::triangles(3, 0), |_| {
            panic!("CPU assembly must not invoke mesh preparation")
        })
    }

    fn texture() -> TextureSource {
        TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            |_| panic!("CPU assembly must not invoke texture preparation"),
        )
    }

    fn coverage() -> MaskSource {
        MaskSource::new(
            MaskDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
            |_| panic!("CPU assembly must not invoke mask preparation"),
        )
    }

    fn translate(x: f32, y: f32) -> Matrix4<f32> {
        Matrix4::new_translation(&nalgebra::Vector3::new(x, y, 0.))
    }

    fn scale(x: f32, y: f32) -> Matrix4<f32> {
        Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(x, y, 1.))
    }

    fn object(draw: &mut Draw<'_>, mesh: &MeshSource, texture: &TextureSource) -> Object {
        Object::new(draw.mesh(mesh), draw.texture(texture), Matrix4::identity())
    }

    fn append_mask(
        frame: &mut Frame,
        mesh: &MeshSource,
        mask: &MaskSource,
        parent: Option<PixelMaskIndex>,
    ) -> PixelMaskIndex {
        let index = PixelMaskIndex(frame.scene.pixel_masks.len() as u32);
        frame.scene.pixel_masks.push(PixelMask {
            mesh: mesh.id(),
            texture: mask.id(),
            transform: Matrix4::identity(),
            parent,
        });
        index
    }

    #[test]
    fn registration_retains_definitions_without_preparing_and_finish_prunes_absent_kinds() {
        let (mesh, texture, mask) = (mesh(), texture(), coverage());
        let mut frame = Frame::default();
        frame.begin();
        {
            let mut draw = frame.draw(Matrix4::identity(), None, 1.);
            draw.mesh(&mesh);
            draw.texture(&texture);
            draw.mask_source(&mask);
            // Duplicate registration has the same logical identity, not a second entry.
            draw.mesh(&mesh.clone());
            draw.texture(&texture.clone());
            draw.mask_source(&mask.clone());
        }
        frame.finish().expect("retention-only frame");
        assert_eq!(frame.scene.resources.len(), 3);
        assert!(frame.scene.phases[0].objects.is_empty());

        frame.begin();
        assert_eq!(frame.scene.resources.len(), 3, "pruning waits for finish");
        frame.finish().expect("empty next frame");
        assert!(frame.scene.resources.is_empty());
        assert!(frame.scene.pixel_masks.is_empty());
        assert_eq!(frame.scene.phases.len(), 1);
        frame
            .finish()
            .expect("finishing a successful frame twice is safe");
    }

    #[test]
    fn references_keep_all_mask_ancestors_even_without_repeat_registration() {
        let (object_mesh, ancestor_mesh) = (mesh(), mesh());
        let (color, stale_color) = (texture(), texture());
        let (outer, inner) = (coverage(), coverage());
        let mut frame = Frame::default();
        frame.begin();
        {
            let mut draw = frame.draw(Matrix4::identity(), None, 1.);
            draw.mesh(&object_mesh);
            draw.mesh(&ancestor_mesh);
            draw.texture(&color);
            draw.texture(&stale_color);
            draw.mask_source(&outer);
            draw.mask_source(&inner);
        }
        frame.finish().expect("definitions retained for next frame");

        frame.begin();
        let root = append_mask(&mut frame, &ancestor_mesh, &outer, None);
        let leaf = append_mask(&mut frame, &object_mesh, &inner, Some(root));
        frame
            .draw(Matrix4::identity(), Some(leaf), 1.)
            .object(Object::new(
                object_mesh.id(),
                color.id(),
                Matrix4::identity(),
            ));
        frame
            .finish()
            .expect("previous definitions are still available");
        assert_eq!(frame.scene.resources.len(), 5);
        assert!(frame.scene.resources.mesh(ancestor_mesh.id()).is_some());
        assert!(frame.scene.resources.mask(outer.id()).is_some());
        assert!(frame.scene.resources.mask(inner.id()).is_some());
        assert!(frame.scene.resources.texture(stale_color.id()).is_none());
        assert_eq!(frame.scene.phases[0].objects[0].mask, Some(leaf));
    }

    #[test]
    fn assembly_error_is_sticky_until_begin_and_failed_finish_still_prunes() {
        let (mesh, color, stale) = (mesh(), texture(), texture());
        let mut frame = Frame::default();
        frame.begin();
        frame.draw(Matrix4::identity(), None, 1.).texture(&stale);
        frame.finish().expect("initial frame");

        frame.begin();
        frame
            .draw(Matrix4::identity(), None, 1.)
            .object(Object::new(mesh.id(), color.id(), Matrix4::identity()));
        let first = frame.finish().expect_err("unregistered resources");
        assert!(
            frame.scene.resources.is_empty(),
            "error does not retain stale entries"
        );
        assert!(frame.scene.phases[0].objects.is_empty());

        // A later, different failure must not overwrite or consume the first one.
        let mut invalid_mask = Object::new(mesh.id(), color.id(), Matrix4::identity());
        invalid_mask.mask = Some(PixelMaskIndex(u32::MAX));
        frame
            .draw(Matrix4::identity(), None, 1.)
            .object(invalid_mask);
        assert_eq!(frame.finish(), Err(first.clone()));
        assert_eq!(frame.finish(), Err(first));

        frame.begin();
        {
            let mut draw = frame.draw(Matrix4::identity(), None, 1.);
            let object = object(&mut draw, &mesh, &color);
            draw.object(object);
        }
        frame.finish().expect("begin resets the failure");
        assert_eq!(frame.scene.phases[0].objects.len(), 1);
    }

    #[test]
    fn conflicting_definition_keeps_original_and_remains_an_error_after_finish() {
        let original = texture();
        let conflicting = TextureSource::with_id(
            original.id(),
            TextureDescriptor::new([2, 1], wgpu::TextureFormat::Rgba8Unorm),
            |_| panic!("assembly does not prepare conflicting definitions"),
        );
        let mut frame = Frame::default();
        frame.begin();
        {
            let mut draw = frame.draw(Matrix4::identity(), None, 1.);
            draw.texture(&original);
            draw.texture(&conflicting);
        }
        let error = frame.finish().expect_err("descriptor conflict");
        assert_eq!(frame.finish(), Err(error));
        assert_eq!(
            frame
                .scene
                .resources
                .texture(original.id())
                .expect("original definition")
                .descriptor(),
            original.descriptor(),
        );
    }

    #[test]
    fn backdrop_boundaries_follow_paint_order_across_independent_writers() {
        let mesh = mesh();
        let colors = [texture(), texture(), texture(), texture(), texture()];
        let mut frame = Frame::default();
        frame.begin();
        {
            let mut writer_a = frame.draw(Matrix4::identity(), None, 1.);
            let background = object(&mut writer_a, &mesh, &colors[0]);
            writer_a.object(background);
            let effect = object(&mut writer_a, &mesh, &colors[1]);
            writer_a.backdrop(effect);
            let foreground = object(&mut writer_a, &mesh, &colors[2]);
            writer_a.object(foreground);
        }
        {
            let mut writer_b = frame.draw(translate(10., 20.), None, 1.);
            let effect = object(&mut writer_b, &mesh, &colors[3]);
            writer_b.backdrop(effect);
            let foreground = object(&mut writer_b, &mesh, &colors[4]);
            writer_b.object(foreground);
        }
        frame.finish().expect("ordered writers");
        let phases: Vec<Vec<_>> = frame
            .scene
            .phases
            .iter()
            .map(|phase| phase.objects.iter().map(|object| object.texture).collect())
            .collect();
        assert_eq!(
            phases,
            vec![
                vec![colors[0].id()],
                vec![colors[1].id(), colors[2].id()],
                vec![colors[3].id(), colors[4].id()],
            ]
        );

        frame.begin();
        {
            let mut draw = frame.draw(Matrix4::identity(), None, 1.);
            // Already retained resource IDs remain ordinary immutable references.
            draw.backdrop(Object::new(mesh.id(), colors[1].id(), Matrix4::identity()));
        }
        frame.finish().expect("only first backdrop remains");
        assert_eq!(frame.scene.phases.len(), 1, "discard inactive old phases");
        assert_eq!(frame.scene.phases[0].objects.len(), 1);
        assert_eq!(frame.scene.resources.len(), 2);
    }

    #[test]
    fn consecutive_backdrops_start_at_phase_zero_and_do_not_clone_resources() {
        let (mesh, color) = (mesh(), texture());
        let mut frame = Frame::default();
        frame.begin();
        {
            let mut draw = frame.draw(Matrix4::identity(), None, 1.);
            for _ in 0..3 {
                let object = object(&mut draw, &mesh, &color);
                draw.backdrop(object);
            }
        }
        frame.finish().expect("three ordered draws");
        assert_eq!(frame.scene.phases.len(), 3);
        assert!(
            frame
                .scene
                .phases
                .iter()
                .all(|phase| phase.objects.len() == 1)
        );
        assert_eq!(
            frame.scene.resources.len(),
            2,
            "phase boundaries do not change IDs"
        );
    }

    #[test]
    fn nested_mask_and_transform_scopes_resolve_once_and_restore_sibling_state() {
        let (mesh, color) = (mesh(), texture());
        let (outer, inner) = (coverage(), coverage());
        let world = translate(100., 200.) * scale(2., 3.);
        let scope = translate(5., 7.);
        let mask_geometry = translate(11., 13.) * scale(50., 60.);
        let geometry = translate(2., 4.);
        let mut frame = Frame::default();
        frame.begin();
        {
            let mut draw = frame.draw(world, None, 0.4);
            draw.masked(&mesh, &outer, mask_geometry, |masked| {
                masked.translated(scope, |translated| {
                    assert_eq!(translated.transform(), world * scope);
                    translated.quad(&color, [8., 9.], geometry, Some(&inner));
                });
                assert_eq!(
                    masked.transform(),
                    world,
                    "mask geometry is not child placement"
                );
                let mut sibling = object(masked, &mesh, &color);
                sibling.opacity = 0.5;
                masked.object(sibling);
            });
            draw.quad(&color, [1., 1.], Matrix4::identity(), None);
        }
        frame.finish().expect("scoped geometry");
        let objects = &frame.scene.phases[0].objects;
        let masks = &frame.scene.pixel_masks;
        assert_eq!(objects.len(), 3);
        assert_eq!(masks.len(), 2);
        assert_eq!(masks[0].transform, world * mask_geometry);
        assert_eq!(masks[0].parent, None);
        assert_eq!(masks[1].transform, world * scope * geometry * scale(8., 9.));
        assert_eq!(masks[1].parent, Some(PixelMaskIndex(0)));
        assert_eq!(objects[0].transform, masks[1].transform);
        assert_eq!(objects[0].mask, Some(PixelMaskIndex(1)));
        assert_eq!(objects[0].opacity, 0.4);
        assert_eq!(objects[1].transform, world);
        assert_eq!(objects[1].mask, Some(PixelMaskIndex(0)));
        assert_eq!(objects[1].opacity, 0.2);
        assert_eq!(objects[2].transform, world);
        assert_eq!(objects[2].mask, None);
    }

    #[test]
    fn explicit_masks_must_preserve_the_inherited_chain_without_duplicate_coverage() {
        // Chain 1 -> 0; chain 2 is unrelated. None means inherit the selected scope.
        for (inherited, explicit, valid, expected) in [
            (Some(0), None, true, Some(0)),
            (Some(0), Some(0), true, Some(0)),
            (Some(0), Some(1), true, Some(1)),
            (None, Some(2), true, Some(2)),
            (Some(0), Some(2), false, None),
            (Some(1), Some(0), false, None),
            (Some(99), Some(2), false, None),
        ] {
            let (mesh, color, coverage) = (mesh(), texture(), coverage());
            let mut frame = Frame::default();
            frame.begin();
            {
                let mut draw = frame.draw(Matrix4::identity(), None, 1.);
                draw.mesh(&mesh);
                draw.texture(&color);
                draw.mask_source(&coverage);
            }
            append_mask(&mut frame, &mesh, &coverage, None);
            append_mask(&mut frame, &mesh, &coverage, Some(PixelMaskIndex(0)));
            append_mask(&mut frame, &mesh, &coverage, None);
            let mut object = Object::new(mesh.id(), color.id(), Matrix4::identity());
            object.mask = explicit.map(PixelMaskIndex);
            frame
                .draw(Matrix4::identity(), inherited.map(PixelMaskIndex), 1.)
                .object(object);
            assert_eq!(
                frame.finish().is_ok(),
                valid,
                "inherited {inherited:?}, explicit {explicit:?}"
            );
            assert_eq!(
                frame.scene.pixel_masks.len(),
                3,
                "no implicit chain copying"
            );
            if valid {
                assert_eq!(
                    frame.scene.phases[0].objects[0].mask,
                    expected.map(PixelMaskIndex)
                );
            } else {
                assert!(frame.scene.phases[0].objects.is_empty());
            }
        }
    }

    #[test]
    fn invalid_mask_indices_parents_and_missing_definitions_fail_without_emitting() {
        for (index, parent, define_mask) in [
            (99, None, true),
            (0, Some(0), true),
            (0, Some(1), true),
            (0, None, false),
        ] {
            let (mesh, color, mask) = (mesh(), texture(), coverage());
            let mut frame = Frame::default();
            frame.begin();
            {
                let mut draw = frame.draw(Matrix4::identity(), None, 1.);
                draw.mesh(&mesh);
                draw.texture(&color);
                if define_mask {
                    draw.mask_source(&mask);
                }
            }
            append_mask(&mut frame, &mesh, &mask, parent.map(PixelMaskIndex));
            frame
                .draw(Matrix4::identity(), Some(PixelMaskIndex(index)), 1.)
                .object(Object::new(mesh.id(), color.id(), Matrix4::identity()));
            assert!(frame.finish().is_err());
            assert!(frame.scene.phases[0].objects.is_empty());
        }
    }

    #[test]
    fn unit_quad_definitions_share_content_identity_and_fast_path_guarantees() {
        let (a, b) = (unit_quad(), unit_quad());
        assert_eq!(a.id(), b.id());
        assert_eq!(a.descriptor(), b.descriptor());
        assert_eq!(a.descriptor().vertex_count, 6);
        assert_eq!(a.descriptor().index_count, 0);
        assert_eq!(a.descriptor().bounds, Some([[0., 0., 0.], [1., 1., 0.]]));
        assert!(a.descriptor().non_overlapping);
    }
}
