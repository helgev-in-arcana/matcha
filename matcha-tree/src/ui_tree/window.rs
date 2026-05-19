use std::sync::Arc;

use parking_lot::Mutex;
use renderer::RenderNode;
use utils::maybe_send_sync::MaybeSendSync;

use crate::ui_tree::{
    context::{UiContext, WindowCtx},
    metrics,
    widget::{View, Widget, WidgetInteractionResult, WidgetPod, WidgetUpdateError},
};
use matcha_window::event::device_event::DeviceEvent;
use matcha_window::window::{Window as OsWindow, WindowConfig, WindowError, WindowId};

// ------
// Window
// ------

/// Declares a window anywhere in the view tree.
///
/// When built, creates a [`WindowWidgetInstance`] and registers it with
/// [`UiContext::register_window_instance`] so that [`UiArch`](super::UiArch) can
/// route events and rendering directly to this window.
pub struct Window {
    pub window_id: String,
    pub config: WindowConfig,
    pub view: Box<dyn View>,
}

impl View for Window {
    fn build(&self, ctx: &UiContext) -> WidgetPod {
        let window = ctx.create_window(&self.config).unwrap();
        let inner_widget = self.view.build(ctx);
        let instance = Arc::new(Mutex::new(WindowWidgetInstance::new(
            window.id(),
            window,
            inner_widget,
        )));
        ctx.register_window_instance(
            Arc::clone(&instance) as Arc<Mutex<dyn AnyWindowWidgetInstance>>
        );
        WidgetPod::new(&self.window_id, WindowWidget { instance })
    }
}

// ------------
// WindowWidget
// ------------

/// The [`Widget`] counterpart of [`Window`].
///
/// Holds the strong [`Arc`] to the [`WindowWidgetInstance`].
/// This is a zero-size widget in the parent's layout 窶・rendering and input for
/// the window's content are handled by [`UiArch`](super::UiArch) directly via
/// the window registry, never through the parent widget tree.
pub struct WindowWidget {
    instance: Arc<Mutex<WindowWidgetInstance>>,
}

impl Widget for WindowWidget {
    type View = Window;

    fn update(&mut self, view: &Window, ctx: &UiContext) -> WidgetInteractionResult {
        // The window already exists; just keep the inner widget in sync.
        // Registration was done in Window::build() and is not repeated here.
        let mut instance = self.instance.lock();
        instance.try_update(view, &ctx)
    }

    fn device_input(
        &mut self,
        _bounds: [f32; 2],
        _event: &DeviceEvent,
        _ctx: &UiContext,
    ) -> WidgetInteractionResult {
        // Input does not cross window boundaries; handled by UiArch per-window.
        WidgetInteractionResult::NoChange
    }

    fn measure(&self, _constraints: &metrics::Constraints, _ctx: &UiContext) -> [f32; 2] {
        // Zero-size: the window occupies no space in the parent layout.
        [0.0, 0.0]
    }

    fn render(&mut self, _bounds: [f32; 2], _ctx: &UiContext) -> RenderNode {
        // Nothing to render in the parent tree; the window draws to its own surface.
        RenderNode::new()
    }
}

// --------------------
// WindowWidgetInstance
// --------------------

/// Live state for one OS window that lives inside the widget tree.
///
/// `window_id` is stored outside any lock because it is immutable after creation.
/// The `widget` field is accessed through the owning [`WindowWidget`]'s methods.
pub struct WindowWidgetInstance {
    window_id: WindowId,
    /// Keeps the OS window alive. Dropping this Arc (when the last strong ref goes away)
    /// triggers `WindowHandle::drop`, which removes the window from `WindowManager`.
    window: OsWindow,
    widget: WidgetPod,
}

impl WindowWidgetInstance {
    pub fn new(window_id: WindowId, window: OsWindow, widget: WidgetPod) -> Self {
        Self {
            window_id,
            window,
            widget,
        }
    }

    pub fn try_update(&mut self, view: &Window, ctx: &UiContext) -> WidgetInteractionResult {
        let s = self.window.inner_size();
        let window_ctx = WindowCtx {
            dpi: self.window.dpi(),
            format: self.window.format(),
            config: self.window.config().clone(),
            inner_size: [s[0] as f32, s[1] as f32],
        };
        let ctx = UiContext {
            event_loop: ctx.event_loop,
            shared: ctx.shared,
            window: Some(&window_ctx),
        };
        match self.widget.try_update(view.view.as_ref(), &ctx) {
            Ok(result) => {
                if matches!(
                    result,
                    WidgetInteractionResult::LayoutNeeded | WidgetInteractionResult::RedrawNeeded
                ) {
                    self.window.request_redraw();
                }
                result
            }
            Err(WidgetUpdateError::TypeMismatch) => {
                self.widget = view.view.build(&ctx);
                self.window.request_redraw();
                WidgetInteractionResult::LayoutNeeded
            }
        }
    }
}

// -------------------------
// AnyWindowWidgetInstance
// -------------------------

/// Type-erased interface for [`WindowWidgetInstance`].
///
/// [`UiArch`](super::UiArch) stores `Weak<Mutex<dyn AnyWindowWidgetInstance>>` in its
/// registry keyed by [`WindowId`]. The strong [`Arc`] lives in the owning [`WindowWidget`];
/// when the window is removed from the view tree the widget is dropped, the `Arc` count
/// reaches zero, and `UiArch`'s `Weak` becomes dead (window is destroyed automatically
/// via [`WindowHandle`]'s [`Drop`] impl).
pub trait AnyWindowWidgetInstance: MaybeSendSync {
    fn window_id(&self) -> WindowId;
    fn size(&self) -> [f32; 2];
    fn device_input(&mut self, event: &DeviceEvent, ctx: &UiContext) -> WidgetInteractionResult;
    fn render(
        &mut self,
        renderer: &renderer::CoreRenderer,
        texture_atlas: &wgpu::Texture,
        stencil_atlas: &wgpu::Texture,
        ctx: &UiContext,
    );
    fn measure(&self, constraints: &metrics::Constraints, ctx: &UiContext) -> [f32; 2];
    fn create_surface(
        &mut self,
        instance: &wgpu::Instance,
        device: &wgpu::Device,
    ) -> Result<(), WindowError>;
    fn destroy_surface(&mut self);
}

impl AnyWindowWidgetInstance for WindowWidgetInstance {
    fn window_id(&self) -> WindowId {
        self.window_id
    }

    fn size(&self) -> [f32; 2] {
        let s = self.window.inner_size();
        [s[0] as f32, s[1] as f32]
    }

    fn device_input(&mut self, event: &DeviceEvent, ctx: &UiContext) -> WidgetInteractionResult {
        let s = self.window.inner_size();
        let window_ctx = WindowCtx {
            dpi: self.window.dpi(),
            format: self.window.format(),
            config: self.window.config().clone(),
            inner_size: [s[0] as f32, s[1] as f32],
        };
        let ctx = UiContext {
            event_loop: ctx.event_loop,
            shared: ctx.shared,
            window: Some(&window_ctx),
        };
        let bounds = self.size();
        let result = self.widget.device_input(bounds, event, &ctx);
        if matches!(
            result,
            WidgetInteractionResult::LayoutNeeded | WidgetInteractionResult::RedrawNeeded
        ) {
            self.window.request_redraw();
        }
        result
    }

    fn render(
        &mut self,
        renderer: &renderer::CoreRenderer,
        texture_atlas: &wgpu::Texture,
        stencil_atlas: &wgpu::Texture,
        ctx: &UiContext,
    ) {
        let size = self.size();
        let s = self.window.inner_size();
        let window_ctx = WindowCtx {
            dpi: self.window.dpi(),
            format: self.window.format(),
            config: self.window.config().clone(),
            inner_size: [s[0] as f32, s[1] as f32],
        };
        let widget_ctx = UiContext {
            event_loop: ctx.event_loop,
            shared: ctx.shared,
            window: Some(&window_ctx),
        };
        let render_node = self.widget.render(size, &widget_ctx);

        let format = self.window.format();

        let device = &ctx.shared.gpu_device;
        let queue = &ctx.shared.gpu_queue;

        let _ = self
            .window
            .surface()
            .rendering_with_surface_texture(device, |view, _texture| {
                let _ = renderer.render(
                    device,
                    queue,
                    format,
                    view,
                    size,
                    &render_node,
                    wgpu::Color {
                        r: 0.1,
                        g: 0.1,
                        b: 0.1,
                        a: 1.0,
                    },
                    texture_atlas,
                    stencil_atlas,
                );
            });
    }

    fn measure(&self, constraints: &metrics::Constraints, ctx: &UiContext) -> [f32; 2] {
        let s = self.window.inner_size();
        let window_ctx = WindowCtx {
            dpi: self.window.dpi(),
            format: self.window.format(),
            config: self.window.config().clone(),
            inner_size: [s[0] as f32, s[1] as f32],
        };
        let ctx = UiContext {
            event_loop: ctx.event_loop,
            shared: ctx.shared,
            window: Some(&window_ctx),
        };
        self.widget.measure(constraints, &ctx)
    }

    fn create_surface(
        &mut self,
        instance: &wgpu::Instance,
        device: &wgpu::Device,
    ) -> Result<(), WindowError> {
        self.window.create_surface(instance, device)
    }

    fn destroy_surface(&mut self) {
        self.window.destroy_surface();
    }
}
