use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use matcha_better_tree::{
    UiTree,
    context::Context,
    metrics::{Bounds, Constraints, Position},
    widget::*,
};
use matcha_window::event::device_event::{
    DeviceEvent, DeviceEventData, DeviceEventState, MouseInput,
};
use nalgebra::{Matrix4, Vector3};
use render_interface::{
    MaskDescriptor, MaskSource, MeshDescriptor, MeshSource, PrepareOutputLayout, Scene,
    TextureDescriptor, TextureSource, Vertex, upload_buffer, upload_texture,
};

#[derive(Default)]
struct Counts {
    render: AtomicUsize,
    layout: AtomicUsize,
    prepare: AtomicUsize,
}

#[derive(Clone)]
struct TestView {
    key: u64,
    position: Position,
    children: Vec<TestView>,
    color: Option<[u8; 4]>,
    mask: Option<u8>,
    backdrop: bool,
    transform: Matrix4<f32>,
    opacity: f32,
    input: InputResult,
    counts: Arc<Counts>,
}

impl TestView {
    fn node(key: u64, color: Option<[u8; 4]>, children: Vec<Self>) -> Self {
        Self {
            key,
            position: Position {
                bounds: bounds(16.0),
                ..Default::default()
            },
            children,
            color,
            mask: None,
            backdrop: false,
            transform: Matrix4::identity(),
            opacity: 1.0,
            input: InputResult::default(),
            counts: Arc::default(),
        }
    }
}

struct TestWidget {
    view: TestView,
    children: Vec<ChildId>,
}

impl View for TestView {
    fn label(&self) -> Option<&str> {
        None
    }
    fn key(&self) -> Option<u64> {
        Some(self.key)
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a>) -> impl Widget<V = Self> {
        TestWidget {
            view: self.clone(),
            children: self.children.iter().map(|child| ctx.child(child)).collect(),
        }
    }
}

impl Widget for TestWidget {
    type V = TestView;
    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a>) -> UpdateResult {
        let layout_changed = self.view.children.len() != view.children.len()
            || self
                .view
                .children
                .iter()
                .zip(&view.children)
                .any(|(a, b)| a.position != b.position);
        let paint_changed = self.view.color != view.color
            || self.view.mask != view.mask
            || self.view.backdrop != view.backdrop
            || self.view.transform != view.transform
            || self.view.opacity != view.opacity;
        self.children = view.children.iter().map(|child| ctx.child(child)).collect();
        self.view = view.clone();
        let mut result = UpdateResult::empty();
        result.set(UpdateResult::LAYOUT, layout_changed);
        result.set(UpdateResult::RENDER, paint_changed);
        result
    }
    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: Context) -> InputResult {
        if self.view.input.update.contains(UpdateResult::RENDER) {
            self.view.color = Some([0, 0, 255, 255]);
        }
        if self.view.input.update.contains(UpdateResult::LAYOUT) {
            for child in &mut self.view.children {
                child.position.affine = translation(20.0, 0.0) * child.position.affine;
            }
        }
        self.view.input
    }
    fn measure(&self, _: Constraints, _: Children<'_>, _: Context) -> Bounds {
        self.view.position.bounds
    }
    fn layout(&self, _: Bounds, mut children: Children<'_>, ctx: Context) {
        self.view.counts.layout.fetch_add(1, Ordering::Relaxed);
        for (id, view) in self.children.iter().zip(&self.view.children) {
            children.layout(*id, view.position, ctx.clone());
        }
    }
    fn uses_backdrop(&self) -> bool {
        self.view.backdrop
    }
    fn render(&self, bounds: Bounds, _: Context) -> Draw {
        self.view.counts.render.fetch_add(1, Ordering::Relaxed);
        let mesh = quad();
        let transform = self.view.transform
            * Matrix4::new_nonuniform_scaling(&Vector3::new(bounds.x, bounds.y, 1.0));
        let mut draw = Draw::default();
        if let Some(rgba) = self.view.color {
            let counts = self.view.counts.clone();
            let texture = if self.view.backdrop {
                // Sample the first pixel of the phase-start image, not this widget's own colors.
                TextureSource::new(
                    TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba16Float),
                    move |c| {
                        counts.prepare.fetch_add(1, Ordering::Relaxed);
                        c.gpu.snapshot.color.copy_to(
                            c.gpu.encoder,
                            &c.target.region,
                            [0, 0],
                            [0, 0],
                            [1, 1],
                        )?;
                        Ok(())
                    },
                )
            } else {
                TextureSource::new(
                    TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
                    move |mut c| {
                        counts.prepare.fetch_add(1, Ordering::Relaxed);
                        upload_texture(&mut c.gpu, &c.target, &rgba)
                    },
                )
                .with_output_layout(PrepareOutputLayout::AnyRegion)
            };
            draw.color.push(Color {
                mesh: mesh.clone(),
                texture,
                transform,
                opacity: self.view.opacity,
            });
        }
        if let Some(value) = self.view.mask {
            draw.mask = Some(Mask {
                mesh,
                transform,
                mask: MaskSource::new(
                    MaskDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
                    move |mut c| upload_texture(&mut c.gpu, &c.target, &[value]),
                ),
            });
        }
        draw
    }
}

fn quad() -> MeshSource {
    let mut descriptor = MeshDescriptor::triangles(6, 0);
    descriptor.bounds = Some([[0.0, 0.0, 0.0], [1.0, 1.0, 0.0]]);
    descriptor.non_overlapping = true;
    MeshSource::new(descriptor, |mut c| {
        let vertices = [
            [0.0, 0.0],
            [0.0, 1.0],
            [1.0, 1.0],
            [0.0, 0.0],
            [1.0, 1.0],
            [1.0, 0.0],
        ]
        .map(|p| Vertex {
            position: [p[0], p[1], 0.0],
            uv: p,
        });
        upload_buffer(
            &mut c.gpu,
            c.target.vertices,
            bytemuck::cast_slice(&vertices),
        )
    })
    .with_output_layout(PrepareOutputLayout::AnyRegion)
}

fn bounds(size: f32) -> Bounds {
    Bounds {
        x: size,
        y: size,
        z: 0.0,
    }
}
fn translation(x: f32, y: f32) -> Matrix4<f32> {
    Matrix4::new_translation(&Vector3::new(x, y, 0.0))
}
fn ready(view: &TestView) -> UiTree {
    let mut tree = UiTree::new();
    tree.update(view);
    tree.layout(bounds(64.0), Context::new());
    tree
}
fn calls(view: &TestView) -> usize {
    view.counts.render.load(Ordering::Relaxed)
}
fn scene(tree: &mut UiTree) -> Scene {
    tree.render(Context::new()).unwrap()
}

#[test]
fn warm_scenes_keep_ids_and_definitions_without_rendering_again() {
    let view = TestView::node(1, Some([255, 0, 0, 255]), vec![]);
    let mut tree = ready(&view);
    let a = scene(&mut tree);
    tree.update(&view);
    let b = scene(&mut tree);
    assert_eq!(calls(&view), 1);
    assert_eq!(a.phases[0].objects[0].mesh, b.phases[0].objects[0].mesh);
    assert_eq!(
        a.phases[0].objects[0].texture,
        b.phases[0].objects[0].texture
    );
    assert_eq!(b.resources.len(), 2);
    assert!(b.resources.mesh(b.phases[0].objects[0].mesh).is_some());
    assert_eq!(
        b.resources
            .texture(b.phases[0].objects[0].texture)
            .unwrap()
            .output_layout(),
        PrepareOutputLayout::AnyRegion
    );
}

#[test]
fn global_epochs_independently_refresh_layout_and_draw_once() {
    for flags in [
        UpdateResult::empty(),
        UpdateResult::LAYOUT,
        UpdateResult::RENDER,
        UpdateResult::LAYOUT | UpdateResult::RENDER,
    ] {
        let view = TestView::node(
            1,
            Some([255, 0, 0, 255]),
            vec![TestView::node(2, Some([0, 255, 0, 255]), vec![])],
        );
        let mut tree = ready(&view);
        let first = scene(&mut tree);
        // Several environmental changes can coalesce before the next pass.
        tree.invalidate_caches(flags);
        tree.invalidate_caches(flags);
        tree.update(&view); // An unchanged update must not erase pending global invalidation.
        for v in [&view, &view.children[0]] {
            assert_eq!(calls(v), 1);
            assert_eq!(v.counts.layout.load(Ordering::Relaxed), 1);
        }
        if flags.contains(UpdateResult::LAYOUT) {
            assert!(matches!(
                tree.render(Context::new()),
                Err(UiError::LayoutRequired)
            ));
        }
        tree.layout(bounds(64.0), Context::new());
        let second = scene(&mut tree);
        tree.layout(bounds(64.0), Context::new());
        let third = scene(&mut tree);
        for v in [&view, &view.children[0]] {
            assert_eq!(
                v.counts.layout.load(Ordering::Relaxed),
                1 + usize::from(flags.contains(UpdateResult::LAYOUT))
            );
            assert_eq!(
                calls(v),
                1 + usize::from(flags.contains(UpdateResult::RENDER))
            );
        }
        for index in 0..2 {
            assert_eq!(
                first.phases[0].objects[index].texture != second.phases[0].objects[index].texture,
                flags.contains(UpdateResult::RENDER)
            );
            assert_eq!(
                second.phases[0].objects[index].texture,
                third.phases[0].objects[index].texture
            );
        }
    }
}

#[test]
fn render_only_update_refreshes_only_the_changed_nodes_ids() {
    let mut view = TestView::node(
        1,
        Some([255, 0, 0, 255]),
        vec![TestView::node(2, Some([0, 255, 0, 255]), vec![])],
    );
    let mut tree = ready(&view);
    let a = scene(&mut tree);
    view.children[0].color = Some([0, 0, 255, 255]);
    tree.update(&view);
    let b = scene(&mut tree); // No layout needed for a render-only update.
    assert_eq!(calls(&view), 1);
    assert_eq!(calls(&view.children[0]), 2);
    assert_eq!(
        a.phases[0].objects[0].texture,
        b.phases[0].objects[0].texture
    );
    assert_ne!(
        a.phases[0].objects[1].texture,
        b.phases[0].objects[1].texture
    );
    assert_ne!(a.phases[0].objects[1].mesh, b.phases[0].objects[1].mesh);
    assert_eq!(view.children[0].counts.layout.load(Ordering::Relaxed), 1);
}

#[test]
fn movement_only_changes_scene_affine_while_bounds_change_rebuilds_drawing() {
    let mut view = TestView::node(
        1,
        None,
        vec![TestView::node(2, Some([255, 0, 0, 255]), vec![])],
    );
    view.children[0].transform = translation(3.0, 4.0);
    let mut tree = ready(&view);
    let a = scene(&mut tree);
    view.children[0].position.affine = translation(20.0, 10.0);
    tree.update(&view);
    assert!(matches!(
        tree.render(Context::new()),
        Err(UiError::LayoutRequired)
    ));
    tree.layout(bounds(64.0), Context::new());
    let b = scene(&mut tree);
    assert_eq!(calls(&view.children[0]), 1);
    assert_eq!(
        a.phases[0].objects[0].texture,
        b.phases[0].objects[0].texture
    );
    assert_eq!(
        b.phases[0].objects[0].transform,
        translation(20.0, 10.0) * a.phases[0].objects[0].transform
    );
    view.children[0].position.bounds = bounds(8.0);
    tree.update(&view);
    tree.layout(bounds(64.0), Context::new());
    let c = scene(&mut tree);
    assert_eq!(calls(&view.children[0]), 2);
    assert_ne!(
        b.phases[0].objects[0].texture,
        c.phases[0].objects[0].texture
    );
}

#[test]
fn nested_masks_use_absolute_transforms_and_shared_local_meshes() {
    let mut view = TestView::node(
        1,
        Some([255, 0, 0, 255]),
        vec![TestView::node(2, Some([0, 255, 0, 255]), vec![])],
    );
    view.mask = Some(255);
    view.transform = translation(1.0, 2.0);
    view.children[0].mask = Some(128);
    view.children[0].position.affine = translation(10.0, 20.0);
    let mut tree = ready(&view);
    let output = scene(&mut tree);
    assert_eq!(output.pixel_masks.len(), 2);
    assert_eq!(output.pixel_masks[0].parent, None);
    assert_eq!(output.pixel_masks[1].parent.unwrap().0, 0);
    assert_eq!(output.phases[0].objects[0].mask.unwrap().0, 0);
    assert_eq!(output.phases[0].objects[1].mask.unwrap().0, 1);
    assert_eq!(output.phases[0].objects[0].mesh, output.pixel_masks[0].mesh);
    assert_eq!(output.phases[0].objects[1].mesh, output.pixel_masks[1].mesh);
    assert_eq!(
        output.pixel_masks[1].transform,
        translation(10.0, 20.0) * Matrix4::new_nonuniform_scaling(&Vector3::new(16.0, 16.0, 1.0))
    );
    assert_eq!(output.resources.len(), 6);
}

#[test]
fn backdrop_subtrees_refresh_and_split_phases_while_other_nodes_reuse_ids() {
    let mut dependent = TestView::node(
        2,
        Some([0; 4]),
        vec![TestView::node(3, Some([0, 255, 0, 255]), vec![])],
    );
    dependent.backdrop = true;
    let view = TestView::node(
        1,
        Some([255, 0, 0, 255]),
        vec![dependent, TestView::node(4, Some([0, 0, 255, 255]), vec![])],
    );
    let mut tree = ready(&view);
    let a = scene(&mut tree);
    let b = scene(&mut tree);
    assert_eq!(
        b.phases.iter().map(|p| p.objects.len()).collect::<Vec<_>>(),
        vec![1, 1, 2]
    );
    assert_eq!(calls(&view), 1);
    assert_eq!(calls(&view.children[0]), 2);
    assert_eq!(calls(&view.children[0].children[0]), 2);
    assert_eq!(calls(&view.children[1]), 1);
    assert_ne!(
        a.phases[1].objects[0].texture,
        b.phases[1].objects[0].texture
    );
    assert_eq!(
        a.phases[2].objects[1].texture,
        b.phases[2].objects[1].texture
    );
}

#[test]
fn render_only_input_invalidates_drawing_even_when_it_stops_propagation() {
    let mut view = TestView::node(1, Some([255, 0, 0, 255]), vec![]);
    view.input = InputResult {
        update: UpdateResult::RENDER,
        propagation: InputPropagation::Stop,
    };
    let mut tree = ready(&view);
    let a = scene(&mut tree);
    let event = DeviceEventState::default()
        .process(DeviceEvent::stateless(DeviceEventData::MouseInput {
            dragging_from_primary: None,
            dragging_from_secondary: None,
            dragging_from_middle: None,
            event: Some(MouseInput::Moved {
                position: [5.0, 5.0],
            }),
        }))
        .unwrap();
    assert_eq!(tree.input(&event, Context::new()), view.input);
    let b = scene(&mut tree);
    assert_ne!(
        a.phases[0].objects[0].texture,
        b.phases[0].objects[0].texture
    );
    assert_eq!(view.counts.layout.load(Ordering::Relaxed), 1);
}

#[test]
fn update_flags_independently_control_child_layout_and_own_drawing() {
    for flags in [
        UpdateResult::empty(),
        UpdateResult::LAYOUT,
        UpdateResult::RENDER,
        UpdateResult::LAYOUT | UpdateResult::RENDER,
    ] {
        let mut view = TestView::node(
            1,
            Some([255, 0, 0, 255]),
            vec![TestView::node(2, Some([0, 255, 0, 255]), vec![])],
        );
        let mut tree = ready(&view);
        let a = scene(&mut tree);
        if flags.contains(UpdateResult::LAYOUT) {
            view.children[0].position.affine = translation(20.0, 0.0);
        }
        if flags.contains(UpdateResult::RENDER) {
            view.color = Some([0, 0, 255, 255]);
        }
        tree.update(&view);
        if flags.contains(UpdateResult::LAYOUT) {
            assert!(matches!(
                tree.render(Context::new()),
                Err(UiError::LayoutRequired)
            ));
            tree.layout(bounds(64.0), Context::new());
        }
        let b = scene(&mut tree);
        assert_eq!(
            view.counts.layout.load(Ordering::Relaxed),
            1 + usize::from(flags.contains(UpdateResult::LAYOUT))
        );
        assert_eq!(
            calls(&view),
            1 + usize::from(flags.contains(UpdateResult::RENDER))
        );
        assert_eq!(calls(&view.children[0]), 1);
        assert_eq!(
            a.phases[0].objects[0].texture != b.phases[0].objects[0].texture,
            flags.contains(UpdateResult::RENDER)
        );
        assert_eq!(
            a.phases[0].objects[1].texture,
            b.phases[0].objects[1].texture
        );
        let movement = if flags.contains(UpdateResult::LAYOUT) {
            translation(20.0, 0.0)
        } else {
            Matrix4::identity()
        };
        assert_eq!(
            b.phases[0].objects[1].transform,
            movement * a.phases[0].objects[1].transform
        );
    }
}

#[test]
fn input_flags_independently_control_layout_and_render_even_on_stop() {
    for flags in [
        UpdateResult::empty(),
        UpdateResult::LAYOUT,
        UpdateResult::RENDER,
        UpdateResult::LAYOUT | UpdateResult::RENDER,
    ] {
        let mut view = TestView::node(
            1,
            Some([255, 0, 0, 255]),
            vec![TestView::node(2, Some([0, 255, 0, 255]), vec![])],
        );
        view.input = InputResult {
            update: flags,
            propagation: InputPropagation::Stop,
        };
        let mut tree = ready(&view);
        let a = scene(&mut tree);
        // Hit only the root so it can move its child without changing its own Bounds.
        let event = DeviceEventState::default()
            .process(DeviceEvent::stateless(DeviceEventData::MouseInput {
                dragging_from_primary: None,
                dragging_from_secondary: None,
                dragging_from_middle: None,
                event: Some(MouseInput::Moved {
                    position: [40.0, 40.0],
                }),
            }))
            .unwrap();
        assert_eq!(tree.input(&event, Context::new()), view.input);
        if flags.contains(UpdateResult::LAYOUT) {
            assert!(matches!(
                tree.render(Context::new()),
                Err(UiError::LayoutRequired)
            ));
            tree.layout(bounds(64.0), Context::new());
        }
        let b = scene(&mut tree);
        assert_eq!(
            view.counts.layout.load(Ordering::Relaxed),
            1 + usize::from(flags.contains(UpdateResult::LAYOUT))
        );
        assert_eq!(
            calls(&view),
            1 + usize::from(flags.contains(UpdateResult::RENDER))
        );
        assert_eq!(calls(&view.children[0]), 1);
        assert_eq!(
            a.phases[0].objects[0].texture != b.phases[0].objects[0].texture,
            flags.contains(UpdateResult::RENDER)
        );
        assert_eq!(
            a.phases[0].objects[1].texture,
            b.phases[0].objects[1].texture
        );
        let movement = if flags.contains(UpdateResult::LAYOUT) {
            translation(20.0, 0.0)
        } else {
            Matrix4::identity()
        };
        assert_eq!(
            b.phases[0].objects[1].transform,
            movement * a.phases[0].objects[1].transform
        );
    }
}

#[test]
fn empty_and_unlaid_out_trees_have_explicit_behavior() {
    let mut tree = UiTree::new();
    assert!(scene(&mut tree).phases.is_empty());
    let view = TestView::node(1, Some([255, 0, 0, 255]), vec![]);
    tree.update(&view);
    assert!(matches!(
        tree.render(Context::new()),
        Err(UiError::LayoutRequired)
    ));
    assert_eq!(calls(&view), 0);
    tree.layout(bounds(64.0), Context::new());
    scene(&mut tree);
    tree.clear();
    assert!(scene(&mut tree).resources.is_empty());
}

// Exercise the complete UiTree -> Scene -> PlainRenderer pipeline on a real device.
// No NOOP fallback: this test verifies pixels, not merely command recording.
#[test]
fn real_gpu_pipeline_caches_masks_and_observes_backdrop_changes() {
    use gpu_utils::gpu::{Gpu, GpuDescriptor};
    use plain_renderer::{PlainRenderer, PlainTarget};
    use render_interface::TextureRegion;

    let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor {
        backends: match std::env::var("MATCHA_TEST_BACKEND").as_deref() {
            Ok("dx12") => wgpu::Backends::DX12,
            Ok("vulkan") => wgpu::Backends::VULKAN,
            _ => wgpu::Backends::PRIMARY,
        },
        ..GpuDescriptor::standard()
    }))
    .expect("real GPU required for the rendering integration test");
    eprintln!("UI renderer adapter: {:?}", gpu.adapter().get_info());
    let (device, queue) = gpu.context().unwrap();
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("UI pipeline output"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let mut renderer = PlainRenderer::new(&device, &queue);
    let draw = |renderer: &mut PlainRenderer, scene: &Scene, clear: wgpu::Color| {
        renderer
            .render(
                scene,
                PlainTarget {
                    region: TextureRegion::whole(&target_view, target.format()).unwrap(),
                    logical_size: [64.0, 64.0],
                    clear,
                    initial: None,
                },
            )
            .unwrap();
    };

    let mut child = TestView::node(2, Some([0, 255, 0, 255]), vec![]);
    child.position.affine = translation(16.0, 16.0);
    child.mask = Some(128);
    let mut view = TestView::node(1, Some([255, 0, 0, 255]), vec![child]);
    view.mask = Some(128);
    let mut tree = ready(&view);
    draw(&mut renderer, &scene(&mut tree), wgpu::Color::BLACK);
    let pixels = read_pixels(&device, &queue, &target);
    assert_pixel(&pixels, 2, 2, [128, 0, 0, 255]);
    assert_pixel(&pixels, 20, 20, [96, 64, 0, 255]);
    draw(&mut renderer, &scene(&mut tree), wgpu::Color::BLACK);
    assert_eq!(renderer.stats().prepared, 0);
    assert_eq!(calls(&view), 1);
    assert_eq!(calls(&view.children[0]), 1);

    view.children[0].color = Some([0, 0, 255, 255]);
    tree.update(&view);
    draw(&mut renderer, &scene(&mut tree), wgpu::Color::BLACK);
    assert_pixel(
        &read_pixels(&device, &queue, &target),
        20,
        20,
        [96, 0, 64, 255],
    );
    assert_eq!(calls(&view), 1);

    let background = TestView::node(2, Some([255, 0, 0, 255]), vec![]);
    let mut reader = TestView::node(3, Some([0; 4]), vec![]);
    reader.backdrop = true;
    reader.position.affine = translation(16.0, 0.0);
    let mut view = TestView::node(1, None, vec![background, reader]);
    let mut tree = ready(&view);
    draw(&mut renderer, &scene(&mut tree), wgpu::Color::BLACK);
    assert_pixel(
        &read_pixels(&device, &queue, &target),
        24,
        8,
        [255, 0, 0, 255],
    );
    view.children[0].color = Some([0, 0, 255, 255]);
    tree.update(&view);
    draw(&mut renderer, &scene(&mut tree), wgpu::Color::BLACK);
    assert_pixel(
        &read_pixels(&device, &queue, &target),
        24,
        8,
        [0, 0, 255, 255],
    );

    // Reordering changes the snapshot even though the reader's own state is unchanged.
    view.children.swap(0, 1);
    tree.update(&view);
    tree.layout(bounds(64.0), Context::new());
    draw(&mut renderer, &scene(&mut tree), wgpu::Color::BLACK);
    assert_pixel(
        &read_pixels(&device, &queue, &target),
        24,
        8,
        [0, 0, 0, 255],
    );

    // Removing the earlier producer and changing an external clear must not reuse old pixels.
    view.children.pop();
    tree.update(&view);
    tree.layout(bounds(64.0), Context::new());
    draw(
        &mut renderer,
        &scene(&mut tree),
        wgpu::Color {
            r: 0.0,
            g: 1.0,
            b: 0.0,
            a: 1.0,
        },
    );
    assert_pixel(
        &read_pixels(&device, &queue, &target),
        24,
        8,
        [0, 255, 0, 255],
    );
    let blue = wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    };
    let last = scene(&mut tree);
    draw(&mut renderer, &last, blue);
    assert_pixel(
        &read_pixels(&device, &queue, &target),
        24,
        8,
        [0, 0, 255, 255],
    );
    renderer.clear_cache();
    draw(&mut renderer, &last, blue);
    assert_pixel(
        &read_pixels(&device, &queue, &target),
        24,
        8,
        [0, 0, 255, 255],
    );
    assert!(futures::executor::block_on(validation.pop()).is_none());
}

fn read_pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("UI pipeline readback"),
        size: 64 * 64 * 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(64),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    buffer.slice(..).get_mapped_range().to_vec()
}

fn assert_pixel(pixels: &[u8], x: usize, y: usize, expected: [u8; 4]) {
    let actual = &pixels[(y * 64 + x) * 4..(y * 64 + x + 1) * 4];
    assert!(
        actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2),
        "at ({x},{y}): {actual:?} != {expected:?}"
    );
}
