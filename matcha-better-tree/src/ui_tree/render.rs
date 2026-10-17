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
    pub fn render(&mut self, ctx: Context) -> Result<Scene, UiError> {
        let mut scene = Scene::default();
        if let Some(root) = self.root {
            self.check_render_layout(root)?;
            scene.phases.push(Phase::default());
            self.render_node(root, Matrix4::identity(), None, false, &ctx, &mut scene)?;
        }
        Ok(scene)
    }

    fn check_render_layout(&self, id: UiId) -> Result<(), UiError> {
        let node = self.node_ref(id).expect("live render tree");
        if node.cache_epochs.layout != self.cache_epochs.layout
            || node
                .placement
                .is_none_or(|p| node.layout_cache != Some(p.bounds))
        {
            return Err(UiError::LayoutRequired);
        }
        for (_, child) in &node.children {
            self.check_render_layout(*child)?;
        }
        Ok(())
    }

    fn render_node(
        &mut self,
        id: UiId,
        parent_transform: Matrix4<f32>,
        parent_mask: Option<PixelMaskIndex>,
        inherited_backdrop: bool,
        ctx: &Context,
        scene: &mut Scene,
    ) -> Result<(), UiError> {
        let epoch = self.cache_epochs.render;
        let node = self.node_ref_mut(id).expect("live render tree");
        node.sync_render_epoch(epoch);
        let placement = node.placement.expect("layout validated before rendering");
        let widget = node.widget.as_ref().expect("widget restored after layout");
        let backdrop = inherited_backdrop || widget.uses_backdrop();
        if backdrop || node.draw_cache.is_none() {
            // ID ownership is isolated from traversal so an explicit caller-owned resource
            // route can later preserve external IDs without rewriting Scene assembly.
            node.draw_cache = Some(resources::manage(
                widget.render(placement.bounds, ctx.clone()),
            )?);
        }
        let draw = node.draw_cache.as_ref().unwrap();
        let world = parent_transform * placement.affine;
        let children = node.children.clone();

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
            scene.phases.last_mut().unwrap().objects.push(Object {
                mesh,
                texture,
                transform: world * color.transform,
                mask,
                opacity: color.opacity,
            });
        }
        for (_, child) in children {
            self.render_node(child, world, mask, backdrop, ctx, scene)?;
        }
        Ok(())
    }
}
