//! Provider-owned logical decoration sources. The final renderer owns GPU
//! placement and submission. Clears and overwrites stay within the assigned
//! output region and preserve neighboring widget images.
//!
//! Natural buffers follow the union of their styles' required regions. Clipped
//! widget decorations intersect that union with the widget's allocation explicitly.
//! Logical extents may be fractional while textures have whole texels: cropping
//! quad UVs to logical_size / texture_size preserves one UI pixel per source texel.
//! Scaling the entire rounded texture into logical bounds would shrink geometry;
//! enlarging the quad instead would leak a ViewportClear outside logical bounds.
use crate::style::Style;
use matcha_tree::ui_tree::{
    context::UiContext,
    metrics::{Constraints, QRect},
};
use render_interface::Draw;
use render_interface::{
    Matrix4, MeshDescriptor, MeshSource, Object, PrepareOutputLayout, TextureDescriptor,
    TextureSource, Vertex, upload_buffer,
};
use std::sync::Arc;

pub struct Buffer {
    style: Vec<Arc<dyn Style>>,
    clip_to_bounds: bool,
    cache: Option<(Vec<u64>, [u32; 2], PrepareOutputLayout, BufferData)>,
    mesh: Option<([u32; 2], MeshSource)>,
}
pub struct BufferData {
    pub texture: TextureSource,
    pub texture_position: QRect,
    /// Unit-position quad whose UV extent excludes rounded texture padding.
    pub mesh: MeshSource,
}
impl Buffer {
    /// Preserve each style's natural coverage, including an offset or overflow.
    pub fn new(style: Vec<Arc<dyn Style>>) -> Self {
        Self {
            style,
            clip_to_bounds: false,
            cache: None,
            mesh: None,
        }
    }
    /// Clip decorations to the widget's allocated bounds. This policy belongs to
    /// the widget, rather than being an implicit restriction of every Buffer.
    pub fn clipped(style: Vec<Arc<dyn Style>>) -> Self {
        Self {
            clip_to_bounds: true,
            ..Self::new(style)
        }
    }
    pub fn is_inside(&self, p: [f32; 2], bounds: [f32; 2], ctx: &UiContext) -> bool {
        if self.clip_to_bounds && !QRect::new([0., 0.], bounds).contains(p) {
            return false;
        }
        self.style.iter().any(|s| s.is_inside(p, bounds, ctx))
    }
    pub fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> Option<&BufferData> {
        if bounds.iter().any(|v| !v.is_finite() || *v <= 0.) {
            self.cache = None;
            return None;
        }
        let constraints = Constraints::from_boundary(bounds);
        let region = output_region(
            self.style
                .iter()
                .filter_map(|style| style.required_region(&constraints, ctx)),
            bounds,
            self.clip_to_bounds,
        );
        let Some(region) = region else {
            self.cache = None;
            return None;
        };
        let logical_size = region.size();
        let size = logical_size.map(|dimension| dimension.ceil() as u32);
        let painters: Vec<_> = self
            .style
            .iter()
            .filter_map(|style| style.prepare(bounds, region.min(), ctx))
            .collect();
        if painters.is_empty() {
            self.cache = None;
            return None;
        }
        let ids: Vec<_> = painters
            .iter()
            .flat_map(|paint| paint.ids().iter().copied())
            .collect();
        let output_layout = crate::style::combined_output_layout(&painters);
        let uv_max = [
            logical_size[0] / size[0] as f32,
            logical_size[1] / size[1] as f32,
        ];
        self.ensure_mesh(uv_max);
        let mesh = &self.mesh.as_ref().expect("quad mesh was prepared").1;
        if self
            .cache
            .as_ref()
            .is_none_or(|(old, old_size, old_layout, _)| {
                *old != ids || *old_size != size || *old_layout != output_layout
            })
        {
            let mut desc = TextureDescriptor::new(size, wgpu::TextureFormat::Rgba16Float);
            desc.usages = wgpu::TextureUsages::RENDER_ATTACHMENT;
            let texture = TextureSource::new(desc, move |mut context| {
                crate::paint::clear(&mut context, [0.; 4])?;
                crate::style::record_all(&painters, context)
            })
            .with_output_layout(output_layout);
            self.cache = Some((
                ids,
                size,
                output_layout,
                BufferData {
                    texture,
                    texture_position: region,
                    mesh: mesh.clone(),
                },
            ));
        } else if let Some((_, _, _, data)) = &mut self.cache {
            data.texture_position = region;
            if data.mesh.id() != mesh.id() {
                data.mesh = mesh.clone();
            }
        }
        self.cache.as_ref().map(|(_, _, _, data)| data)
    }
    fn ensure_mesh(&mut self, uv_max: [f32; 2]) {
        let key = uv_max.map(f32::to_bits);
        if self.mesh.as_ref().is_some_and(|(old, _)| *old == key) {
            return;
        }
        if uv_max == [1., 1.] {
            // Integer regions use the same immutable quad definition as direct
            // framework drawing and clipping, rather than one copy per widget.
            self.mesh = Some((key, render_interface::unit_quad()));
            return;
        }
        let vertices = quad_vertices(uv_max);
        let mut descriptor = MeshDescriptor::triangles(6, 0);
        descriptor.bounds = Some([[0., 0., 0.], [1., 1., 0.]]);
        descriptor.non_overlapping = true;
        self.mesh = Some((
            key,
            MeshSource::new(descriptor, move |mut context| {
                upload_buffer(
                    &mut context.gpu,
                    context.target.vertices,
                    bytemuck::cast_slice(&vertices),
                )
            })
            .with_output_layout(PrepareOutputLayout::AnyRegion),
        ));
    }
    pub fn paint(&mut self, bounds: [f32; 2], ctx: &UiContext, draw: &mut Draw<'_>) {
        if let Some(data) = self.render(bounds, ctx) {
            let mesh = draw.mesh(&data.mesh);
            let texture = draw.texture(&data.texture);
            let [x, y] = data.texture_position.min();
            let [width, height] = data.texture_position.size();
            let transform = Matrix4::new_translation(&nalgebra::Vector3::new(x, y, 0.))
                * Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(width, height, 1.));
            draw.object(Object::new(mesh, texture, transform));
        }
    }
}

fn output_region(
    regions: impl Iterator<Item = QRect>,
    bounds: [f32; 2],
    clipped: bool,
) -> Option<QRect> {
    let region = regions
        .filter(|region| region.area() > 0.)
        .reduce(|a, b| a.union(&b))?;
    if !clipped {
        return Some(region);
    }
    let clip = QRect::new([0., 0.], bounds);
    let min = [
        region.min_x().max(clip.min_x()),
        region.min_y().max(clip.min_y()),
    ];
    let max = [
        region.max_x().min(clip.max_x()),
        region.max_y().min(clip.max_y()),
    ];
    (max[0] > min[0] && max[1] > min[1])
        .then(|| QRect::new(min, [max[0] - min[0], max[1] - min[1]]))
}

fn quad_vertices(uv_max: [f32; 2]) -> [Vertex; 6] {
    [[0., 0.], [0., 1.], [1., 1.], [0., 0.], [1., 1.], [1., 0.]].map(|position| Vertex {
        position: [position[0], position[1], 0.],
        uv: [position[0] * uv_max[0], position[1] * uv_max[1]],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_regions_preserve_offsets_and_clipping_is_explicit() {
        let regions = [
            QRect::new([-2., 3.], [4., 2.]),
            QRect::new([9., 4.], [5., 3.]),
        ];
        let natural = output_region(regions.into_iter(), [10., 8.], false).expect("natural union");
        assert_eq!(natural.min(), [-2., 3.]);
        assert_eq!(natural.size(), [16., 4.]);
        let clipped =
            output_region(regions.into_iter(), [10., 8.], true).expect("visible intersection");
        assert_eq!(clipped.min(), [0., 3.]);
        assert_eq!(clipped.size(), [10., 4.]);
        assert!(
            output_region(
                [QRect::new([20., 0.], [2., 2.])].into_iter(),
                [10., 8.],
                true
            )
            .is_none()
        );
    }

    #[test]
    fn fractional_uv_extent_preserves_internal_pixel_coordinates() {
        let logical = [10.5, 7.25];
        let texels = [11., 8.];
        let uv_max = [logical[0] / texels[0], logical[1] / texels[1]];
        let vertices = quad_vertices(uv_max);
        assert_eq!(vertices[2].position, [1., 1., 0.]);
        assert_eq!(vertices[2].uv, uv_max);
        for (position, axis) in [(5., 0), (3., 1)] {
            let sampled_texel = position / logical[axis] * uv_max[axis] * texels[axis];
            assert!((sampled_texel - position).abs() < 1e-5);
        }
        // The quad's geometry still spans exactly the logical extent, so a
        // full-texture clear does not acquire the ceil-rounded extra width.
        assert_eq!(vertices[2].position[0] * logical[0], 10.5);
    }

    #[test]
    fn quad_mesh_identity_tracks_uv_content_independently_of_image_contents() {
        let mut buffer = Buffer::new(Vec::new());
        buffer.ensure_mesh([1., 1.]);
        let first = buffer.mesh.as_ref().expect("mesh").1.id();
        buffer.ensure_mesh([1., 1.]);
        assert_eq!(buffer.mesh.as_ref().expect("reused mesh").1.id(), first);
        buffer.ensure_mesh([10.5 / 11., 1.]);
        assert_ne!(
            buffer.mesh.as_ref().expect("different uv mesh").1.id(),
            first
        );
    }

    #[test]
    fn integer_regions_share_the_framework_quad_between_buffers() {
        let mut first = Buffer::new(Vec::new());
        let mut second = Buffer::clipped(Vec::new());
        first.ensure_mesh([1., 1.]);
        second.ensure_mesh([1., 1.]);
        let shared = render_interface::unit_quad().id();
        assert_eq!(first.mesh.as_ref().expect("first quad").1.id(), shared);
        assert_eq!(second.mesh.as_ref().expect("second quad").1.id(), shared);
    }
}
