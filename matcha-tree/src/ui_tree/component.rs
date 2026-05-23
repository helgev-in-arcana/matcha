use std::sync::Arc;

use renderer::RenderNode;

use super::widget::{View, Widget, WidgetInteractionResult, WidgetPod};
use crate::ui_tree::{
    context::{AppContext, UiContext},
    metrics,
    widget::WidgetUpdateError,
};
use matcha_window::event::{device_event::DeviceEvent, window_event::WindowEvent};
use matcha_window::window::WindowId;

// ----------------------------------------------------------------------------
// Component
// ----------------------------------------------------------------------------

/// A trait for building stateful UI components.
///
/// # Lifecycle
///
/// 1. [`setup`](Component::setup) is called once when the component is first attached.
/// 2. [`view`](Component::view) is called to build the widget tree.
/// 3. [`update`](Component::update) is called when a discrete [`Message`](Component::Message)
///    arrives from the application layer.
///
/// # State management
///
/// State is held as [`SharedValue`](shared_buffer::SharedValue) fields.
/// Calling `SharedValue::store()` automatically signals the event loop to schedule
/// a redraw. If no `store()` is called, no redraw is triggered.
///
/// All methods take `&self`, so a component can be shared as `Arc<C>` with
/// the backend without requiring exclusive access.
///
/// # Spawning tasks
///
/// Use `ctx.runtime_handle().spawn(...)` to launch background tasks.
///
/// # Events
///
/// Widgets emit events via `ctx.emit_event(Box<dyn Any + Send>)` rather than
/// returning typed events. The application layer receives and downcasts them.
pub trait Component: utils::MaybeSendSync + 'static {
    /// Discrete commands delivered from the application layer.
    type Message: utils::MaybeSendSync + 'static;

    // -----------------
    // Lifecycle methods
    // -----------------

    /// Called once when the component is first attached to the widget tree.
    fn init(&self, ctx: &AppContext);

    /// Called when the application is resumed.
    fn resumed(&self, ctx: &AppContext);

    /// Called when the application is suspended.
    fn suspended(&self, ctx: &AppContext);

    /// Called when the application is exiting.
    fn exiting(&self, ctx: &AppContext);

    // -----------------
    // Window Event Handling
    // -----------------

    /// Called for every window event.
    fn window_event(&self, window_id: WindowId, event: WindowEvent, ctx: &AppContext) {
        // TODO: Prepare a default implementation to exit on close request.
    }

    // -----------------
    // Update methods
    // -----------------

    /// Called when a discrete [`Message`](Component::Message) is received.
    ///
    /// Typically used to trigger background tasks or write to [`SharedValue`](shared_buffer::SharedValue)
    /// fields. A redraw occurs only if `store()` is called.
    fn update(&self, message: Self::Message, ctx: &UiContext);

    /// Builds the view tree from the current state.
    fn view(&self, ctx: &UiContext) -> Box<dyn View>;

    /// Handles a raw device event (keyboard, mouse, etc.). Default implementation is a no-op.
    fn input(&self, device_event: &DeviceEvent, ctx: &UiContext) {
        let _ = (device_event, ctx);
    }
}

// ----------------------------------------------------------------------------
// ComponentPod
// ----------------------------------------------------------------------------

/// Owns a [`Component`].
///
/// Held by the UI framework. Use [`ComponentPod::arc()`] to share the component
/// state with the backend.
pub struct ComponentPod<C: Component> {
    label: Option<String>,
    component: Arc<C>,
}

impl<C: Component> ComponentPod<C> {
    pub fn new(label: Option<&str>, component: C) -> Self {
        Self {
            label: label.map(|s| s.to_string()),
            component: Arc::new(component),
        }
    }

    /// Returns a cloned `Arc` to the component.
    ///
    /// The backend holds this `Arc` and writes to [`SharedValue`](shared_buffer::SharedValue)
    /// fields directly to update UI state.
    pub fn arc(&self) -> Arc<C> {
        self.component.clone()
    }
}

impl<C: Component> ComponentPod<C> {
    pub fn init(&self, ctx: &AppContext) {
        self.component.init(ctx);
    }

    pub fn resumed(&self, ctx: &AppContext) {
        self.component.resumed(ctx);
    }

    pub fn suspended(&self, ctx: &AppContext) {
        self.component.suspended(ctx);
    }

    pub fn exiting(&self, ctx: &AppContext) {
        self.component.exiting(ctx);
    }
}

impl<C: Component> ComponentPod<C> {
    /// Delivers a discrete [`Message`](Component::Message) to the component.
    pub fn update(&self, message: C::Message, ctx: &UiContext) {
        self.component.update(message, ctx);
    }

    /// Builds a [`ComponentView`] from the current state.
    pub fn view(&self, ctx: &UiContext) -> ComponentView<C> {
        ComponentView {
            label: self.label.clone(),
            component: self.component.clone(),
            inner_view: self.component.view(ctx),
        }
    }
}

// ----------------------------------------------------------------------------
// ComponentView
// ----------------------------------------------------------------------------

pub struct ComponentView<C: Component> {
    label: Option<String>,
    component: Arc<C>,
    inner_view: Box<dyn View>,
}

impl<C: Component> View for ComponentView<C> {
    fn build(&self, ctx: &UiContext) -> WidgetPod {
        WidgetPod::new(
            self.label.as_deref(),
            ComponentWidget {
                component: self.component.clone(),
                inner_widget: self.inner_view.build(ctx),
            },
        )
    }
}

// ----------------------------------------------------------------------------
// ComponentWidget
// ----------------------------------------------------------------------------

struct ComponentWidget<C: Component> {
    component: Arc<C>,
    inner_widget: WidgetPod,
}

impl<C: Component> Widget for ComponentWidget<C> {
    type View = ComponentView<C>;

    fn update(&mut self, view: &Self::View, ctx: &UiContext) -> WidgetInteractionResult {
        match self.inner_widget.try_update(view.inner_view.as_ref(), ctx) {
            Ok(interaction_result) => interaction_result,
            Err(WidgetUpdateError::TypeMismatch) => {
                self.inner_widget = view.inner_view.build(ctx);
                WidgetInteractionResult::LayoutNeeded
            }
        }
    }

    fn device_input(
        &mut self,
        bounds: [f32; 2],
        event: &DeviceEvent,
        ctx: &UiContext,
    ) -> WidgetInteractionResult {
        self.component.input(event, ctx);
        self.inner_widget.device_input(bounds, event, ctx)
    }

    fn is_inside(&self, bounds: [f32; 2], position: [f32; 2], ctx: &UiContext) -> bool {
        self.inner_widget.is_inside(bounds, position, ctx)
    }

    fn measure(&self, constraints: &metrics::Constraints, ctx: &UiContext) -> [f32; 2] {
        self.inner_widget.measure(constraints, ctx)
    }

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode {
        self.inner_widget.render(bounds, ctx)
    }
}
