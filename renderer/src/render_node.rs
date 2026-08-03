use gpu_utils::texture_atlas;
use smallvec::SmallVec;
use std::sync::Arc;

const SMALLVEC_INLINE_CAPACITY: usize = 16;

/// Represents a render tree node that contains drawing information for the renderer.
///
/// Note: Coordinates used by the Dom/Widget/Style APIs are in pixels with the origin at the
/// top-left of the window and the Y axis pointing downwards. The renderer is responsible for
/// converting these coordinates (origin, Y direction, and scale) into normalized device
/// coordinates (NDC) required by the GPU/backend.
///
/// The RenderNode stores textures, stencil information, and child elements along with
/// per-node transform matrices. Transforms are applied by the renderer when generating GPU
/// draw calls.
///
/// A node's stencil masks that node alone — see [`RenderNode::with_stencil`].
#[derive(Debug, Clone)]
pub struct RenderNode {
    texture_and_position: Option<(texture_atlas::AtlasRegion, nalgebra::Matrix4<f32>)>,
    stencil_and_position: Option<(texture_atlas::AtlasRegion, nalgebra::Matrix4<f32>)>,

    child_elements: SmallVec<[(Arc<RenderNode>, nalgebra::Matrix4<f32>); SMALLVEC_INLINE_CAPACITY]>,
}

impl Default for RenderNode {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderNode {
    pub fn new() -> Self {
        Self {
            texture_and_position: None,
            stencil_and_position: None,
            child_elements: SmallVec::new(),
        }
    }

    pub(crate) fn texture(&self) -> Option<&(texture_atlas::AtlasRegion, nalgebra::Matrix4<f32>)> {
        self.texture_and_position.as_ref()
    }

    pub(crate) fn stencil(&self) -> Option<&(texture_atlas::AtlasRegion, nalgebra::Matrix4<f32>)> {
        self.stencil_and_position.as_ref()
    }

    pub(crate) fn child_elements(&self) -> &[(Arc<RenderNode>, nalgebra::Matrix4<f32>)] {
        &self.child_elements
    }

    pub fn with_texture(
        mut self,
        texture: texture_atlas::AtlasRegion,
        texture_size: [f32; 2],
        texture_position: nalgebra::Matrix4<f32>,
    ) -> Self {
        let scale_matrix = nalgebra::Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(
            texture_size[0],
            texture_size[1],
            1.0,
        ));
        self.texture_and_position = Some((texture, texture_position * scale_matrix));
        self
    }

    /// Attach a coverage mask that applies to **this node only**, not to its
    /// children.
    ///
    /// This is the per-object mask: a glyph's coverage bitmap, not a clip.
    /// Masks that should also apply to a subtree are declared outside the tree
    /// instead, as `MaskNode`s passed alongside the nodes to
    /// [`CoreRenderer::render_flat`](crate::core_renderer::CoreRenderer::render_flat)
    /// — a render tree is flattened per widget entity, so it cannot express
    /// nesting *between* entities in the first place, which is exactly where
    /// clipping lives.
    pub fn with_stencil(
        mut self,
        stencil: texture_atlas::AtlasRegion,
        stencil_size: [f32; 2],
        stencil_position: nalgebra::Matrix4<f32>,
    ) -> Self {
        let scale_matrix = nalgebra::Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(
            stencil_size[0],
            stencil_size[1],
            1.0,
        ));
        self.stencil_and_position = Some((stencil, stencil_position * scale_matrix));
        self
    }

    pub fn push_child(
        &mut self,
        child: impl Into<Arc<RenderNode>>,
        transform: nalgebra::Matrix4<f32>,
    ) {
        self.child_elements.push((child.into(), transform));
    }

    pub fn add_child(
        mut self,
        child: impl Into<Arc<RenderNode>>,
        transform: nalgebra::Matrix4<f32>,
    ) -> Self {
        self.child_elements.push((child.into(), transform));
        self
    }
}

impl RenderNode {
    pub fn count(&self) -> usize {
        let mut count = 1; // Count this node
        for (child, _) in &self.child_elements {
            count += child.count();
        }
        count
    }
}
