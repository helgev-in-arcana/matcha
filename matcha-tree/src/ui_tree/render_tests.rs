//! Native source emission through WidgetPod. The noop adapter exercises the
//! renderer callbacks and cache ownership without opening a window or using a GPU.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use dashmap::DashMap;
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use plain_renderer::{PlainRenderer, PlainTarget};
use render_interface::{Draw, Frame};
use render_interface::{Matrix4, TextureDescriptor, TextureSource, upload_texture};

use super::{
    context::{EventSender, SharedCtx, UiContext},
    metrics::Constraints,
    runtime::Runtime,
    sub_widgets::SubWidgetsVec,
    widget::{View, Widget, WidgetInteractionResult, WidgetPod},
};
use matcha_window::event::device_event::DeviceEvent;

fn with_context(run: impl FnOnce(&UiContext<'_>)) {
    let runtime = Runtime::from_tokio(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime"),
    );
    let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor {
        required_features: wgpu::Features::empty(),
        ..GpuDescriptor::noop()
    }))
    .expect("noop GPU context");
    let (gpu_device, gpu_queue) = gpu.context().expect("new device is available");
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let sender = EventSender::new(sender);
    let registry = DashMap::new();
    let shared = SharedCtx {
        runtime_handle: runtime.handle().clone(),
        event_sender: &sender,
        window_registry: &registry,
        gpu_instance: gpu.instance(),
        gpu_device,
        gpu_queue,
        surface_creation_permitted: false,
    };
    let ctx = UiContext {
        shared: &shared,
        event_loop: None,
        window: None,
    };
    run(&ctx);
}

#[derive(Clone)]
struct ProbeView {
    color: u8,
    emissions: Arc<AtomicUsize>,
    preparations: Arc<AtomicUsize>,
}

fn source(color: u8, preparations: Arc<AtomicUsize>) -> TextureSource {
    TextureSource::new(
        TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
        move |mut ctx| {
            preparations.fetch_add(1, Ordering::Relaxed);
            upload_texture(&mut ctx.gpu, &ctx.target, &[color, 0, 0, 255])
        },
    )
}

impl View for ProbeView {
    fn build(&self, _ctx: &UiContext) -> WidgetPod {
        WidgetPod::new(
            "probe",
            ProbeWidget {
                view: self.clone(),
                source: source(self.color, self.preparations.clone()),
            },
        )
    }
}

struct ProbeWidget {
    view: ProbeView,
    source: TextureSource,
}

impl Widget for ProbeWidget {
    type View = ProbeView;

    fn update(&mut self, view: &ProbeView, _ctx: &UiContext) -> WidgetInteractionResult {
        if self.view.color == view.color {
            return WidgetInteractionResult::NoChange;
        }
        self.view = view.clone();
        self.source = source(view.color, view.preparations.clone());
        WidgetInteractionResult::RedrawNeeded
    }

    fn device_input(
        &mut self,
        _bounds: [f32; 2],
        _event: &DeviceEvent,
        _ctx: &UiContext,
    ) -> WidgetInteractionResult {
        WidgetInteractionResult::NoChange
    }

    fn measure(&self, _constraints: &Constraints, _ctx: &UiContext) -> [f32; 2] {
        [8., 8.]
    }

    fn render(&mut self, bounds: [f32; 2], _ctx: &UiContext, draw: &mut Draw<'_>) {
        self.view.emissions.fetch_add(1, Ordering::Relaxed);
        draw.quad(&self.source, bounds, Matrix4::identity(), None);
    }
}

fn emit(pod: &mut WidgetPod, frame: &mut Frame, ctx: &UiContext, x: f32, size: [f32; 2]) {
    frame.begin();
    {
        let mut draw = frame.draw(Matrix4::identity(), None, 1.0);
        let mut child_draw =
            draw.transformed(Matrix4::new_translation(&nalgebra::Vector3::new(x, 0., 0.)));
        pod.render(size, ctx, &mut child_draw);
    }
    frame.finish().expect("valid emitted frame");
}

#[test]
fn clean_widgets_emit_each_frame_while_the_backend_reuses_source_content() {
    with_context(|ctx| {
        let view = ProbeView {
            color: 200,
            emissions: Arc::new(AtomicUsize::new(0)),
            preparations: Arc::new(AtomicUsize::new(0)),
        };
        let mut pod = view.build(ctx);
        let mut frame = Frame::default();
        let mut renderer = PlainRenderer::new(ctx.gpu_device(), ctx.gpu_queue());
        let target = ctx.gpu_device().create_texture(&wgpu::TextureDescriptor {
            label: Some("tree direct scene test"),
            size: wgpu::Extent3d {
                width: 32,
                height: 32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let target_view = target.create_view(&Default::default());
        let render = |renderer: &mut PlainRenderer, frame: &Frame| {
            renderer
                .render(
                    &frame.scene,
                    PlainTarget {
                        region: render_interface::TextureRegion::whole(
                            &target_view,
                            wgpu::TextureFormat::Rgba8Unorm,
                        )
                        .expect("whole output region"),
                        viewport: [32., 32.],
                        clear: wgpu::Color::TRANSPARENT,
                        initial: None,
                    },
                )
                .expect("borrowed widget scene renders");
        };
        assert!(pod.need_redraw());
        emit(&mut pod, &mut frame, ctx, 0., [8., 8.]);
        assert!(!pod.need_redraw());
        let original_id = frame.scene.phases[0].objects[0].texture;
        assert_eq!(view.preparations.load(Ordering::Relaxed), 0);
        render(&mut renderer, &frame);
        assert_eq!(view.preparations.load(Ordering::Relaxed), 1);

        emit(&mut pod, &mut frame, ctx, 10., [12., 8.]);
        assert_eq!(view.emissions.load(Ordering::Relaxed), 2);
        assert_eq!(frame.scene.phases[0].objects.len(), 1);
        let object = &frame.scene.phases[0].objects[0];
        assert_eq!(object.texture, original_id);
        assert_eq!(object.transform[(0, 3)], 10.);
        assert_eq!(object.transform[(0, 0)], 12.);
        render(&mut renderer, &frame);
        assert_eq!(renderer.stats().prepared, 0);
        assert_eq!(view.preparations.load(Ordering::Relaxed), 1);

        let mut changed = view.clone();
        changed.color = 80;
        assert!(matches!(
            pod.try_update(&changed, ctx),
            Ok(WidgetInteractionResult::RedrawNeeded)
        ));
        assert!(pod.need_redraw());
        emit(&mut pod, &mut frame, ctx, 10., [12., 8.]);
        assert_ne!(frame.scene.phases[0].objects[0].texture, original_id);
        assert_eq!(frame.scene.resources.texture_ids().count(), 1);
        render(&mut renderer, &frame);
        assert_eq!(view.preparations.load(Ordering::Relaxed), 2);
    });
}

#[test]
fn child_content_changes_request_redraw_without_a_cached_render_tree() {
    with_context(|ctx| {
        let mut view = ProbeView {
            color: 200,
            emissions: Arc::new(AtomicUsize::new(0)),
            preparations: Arc::new(AtomicUsize::new(0)),
        };
        let mut children = SubWidgetsVec::new();
        let id = SubWidgetsVec::<()>::hash_id("probe");
        assert!(matches!(
            children.update([(id, &view as &dyn View, ())], ctx),
            WidgetInteractionResult::LayoutNeeded
        ));
        view.color = 80;
        assert!(matches!(
            children.update([(id, &view as &dyn View, ())], ctx),
            WidgetInteractionResult::RedrawNeeded
        ));
        assert!(matches!(
            children.update([(id, &view as &dyn View, ())], ctx),
            WidgetInteractionResult::NoChange
        ));
    });
}
