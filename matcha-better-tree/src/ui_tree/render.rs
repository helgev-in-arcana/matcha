//! Widget-local draw caching and lowering to the renderer's absolute Scene contract.
use render_interface::{Object, Phase, PixelMask, PixelMaskIndex, Scene};

use super::*;

mod resources;

impl UiTree {
    /// Build a complete Scene for PlainRenderer (or another render-interface implementation).
    /// Call layout after geometry updates. This method does not schedule layout or GPU work.
    /// Pure draw definitions/IDs are reused; Backdrop subtrees are refreshed every call.
    /// Rebuild the Scene when its external background or target mapping changes: replaying a
    /// previously returned Scene keeps its immutable IDs, including those of snapshot readers.
    pub fn render(&mut self, ctx: &mut Context) -> Result<Scene, UiError> {
        match self.root {
            Some(root) => self.render_from(root, ctx),
            None => Ok(Scene::default()),
        }
    }

    pub(super) fn render_from(&mut self, root: UiId, ctx: &mut Context) -> Result<Scene, UiError> {
        self.check_render_layout(root, ctx)?;
        let mut scene = Scene::default();
        scene.phases.push(Phase::default());
        self.render_node(root, Matrix4::identity(), None, false, ctx, &mut scene)?;
        Ok(scene)
    }

    pub(super) fn check_render_layout(&self, id: UiId, ctx: &mut Context) -> Result<(), UiError> {
        let node = self.node_ref(id).expect("live render tree");
        let mut scoped = node.scoped_context(ctx);
        let ctx = &mut scoped;
        if node.window.is_some() {
            return Ok(());
        }
        if node
            .placement
            .is_none_or(|p| node.layout_cache.get(&p.bounds).is_none())
        {
            return Err(UiError::LayoutRequired);
        }
        for (_, child) in &node.children {
            self.check_render_layout(*child, ctx)?;
        }
        Ok(())
    }

    pub(super) fn render_node(
        &mut self,
        id: UiId,
        parent_transform: Matrix4<f32>,
        parent_mask: Option<PixelMaskIndex>,
        inherited_backdrop: bool,
        ctx: &mut Context,
        scene: &mut Scene,
    ) -> Result<(), UiError> {
        let node = self.node_ref_mut(id).expect("live render tree");
        let mut scoped = node.scoped_context(ctx);
        let ctx = &mut scoped;
        if node.window.is_some() {
            return Ok(());
        }
        let placement = node.placement.expect("layout validated before rendering");
        let widget = node.widget.as_ref().expect("widget restored after layout");
        let backdrop = inherited_backdrop || widget.uses_backdrop();
        let world = parent_transform * placement.affine;
        let child_count = node.children.len();
        let env_reads = &mut node.env_reads;
        let draw = node.render_cache.get_or_try_insert_with((), backdrop, || {
            let (draw, reads) = ctx.observe_env_reads(|ctx| widget.render(placement.bounds, ctx));
            // Retain direct reads even if resource preparation rejects the Draw.
            env_reads.record(ReadPhase::Render, reads);
            resources::manage(draw)
        })?;

        // All generators for this widget see the snapshot before its own colors. Later
        // dependent children get a new phase after those colors. Empty phases are reused.
        if backdrop
            && scene
                .phases
                .last()
                .is_some_and(|phase| !phase.objects.is_empty())
        {
            scene.phases.push(Phase::default());
        }
        let mask = if let Some(mask) = &draw.mask {
            let mesh = scene.resources.share_mesh(&mask.mesh)?;
            let texture = scene.resources.share_mask(&mask.mask)?;
            let index = PixelMaskIndex(
                u32::try_from(scene.pixel_masks.len()).map_err(|_| UiError::MaskIndexOverflow)?,
            );
            scene.pixel_masks.push(PixelMask {
                mesh,
                texture,
                transform: world * mask.transform,
                parent: parent_mask,
            });
            Some(index)
        } else {
            parent_mask
        };
        for color in &draw.color {
            let mesh = scene.resources.share_mesh(&color.mesh)?;
            let texture = scene.resources.share_texture(&color.texture)?;
            scene
                .phases
                .last_mut()
                .expect("scene starts with a phase")
                .objects
                .push(Object {
                    mesh,
                    texture,
                    transform: world * color.transform,
                    mask,
                    opacity: color.opacity,
                });
        }
        // Render callbacks cannot change topology. Copy one ID after each short borrow
        // instead of allocating a snapshot of the children for every rendered node.
        for index in 0..child_count {
            let child = self.node_ref(id).expect("live render tree").children[index].1;
            self.render_node(child, world, mask, backdrop, ctx, scene)?;
        }
        Ok(())
    }
}
