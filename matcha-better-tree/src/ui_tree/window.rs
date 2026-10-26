//! Windows are ordinary declarations in the application's single UI tree.
//!
//! The registry owns native window instances and records their arena locations and
//! native event routes. WindowWidget owns event metrics; UiTree invalidates caches.
//! UiTree's window entry points scope Context to that window; parent rendering and
//! input stop at native window boundaries. App lifecycle scheduling is still separate.

use std::any::Any;
use std::marker::PhantomData;

use matcha_window::event::device_event::DeviceEvent;
use matcha_window::event::window_event::WindowEvent;
use matcha_window::window::{Fullscreen, Position, Size, Theme, Window as OsWindow, WindowButtons};

use crate::metrics::{Bounds, Constraints, Point};
use crate::ui_tree::context::Context;
use crate::ui_tree::widget::{Draw, InputResult, UpdateResult, View, Widget};
use crate::ui_tree::{ChildId, Children, DeclContext};

mod registry;
mod tree;
pub use registry::{WindowInstance, WindowInstanceId, WindowRegistry};

/// Immutable registration handle. Construction fixes both identity and the concrete widget type.
/// Only this boundary knows how to downcast; tree traversal uses its operations.
#[derive(Clone, Copy)]
pub(crate) struct WindowBinding {
    id: WindowInstanceId,
    state: fn(&dyn Any) -> WindowState,
    event: fn(&mut dyn Any, &WindowEvent) -> UpdateResult,
}

struct WindowState {
    content: ChildId,
    dpi: Option<f64>,
    viewport: Option<[f32; 2]>,
}

pub(crate) fn scoped_context<'a>(
    binding: Option<WindowBinding>,
    widget: &dyn Any,
    parent: &'a mut Context<'_>,
) -> Context<'a> {
    match binding {
        Some(binding) => binding.content_context(widget, parent).1,
        None => parent.reborrow(),
    }
}

impl WindowBinding {
    pub(crate) fn new<V: View>(id: WindowInstanceId) -> Self {
        Self {
            id,
            state: |widget| {
                let widget = widget
                    .downcast_ref::<WindowWidget<V>>()
                    .expect("registered window widget type matches");
                WindowState {
                    content: widget.content,
                    dpi: widget.dpi,
                    viewport: widget.viewport,
                }
            },
            event: |widget, event| {
                widget
                    .downcast_mut::<WindowWidget<V>>()
                    .expect("registered window widget type matches")
                    .handle_window_event(event)
            },
        }
    }

    pub(crate) fn id(self) -> WindowInstanceId {
        self.id
    }

    pub(crate) fn content_context<'a>(
        self,
        widget: &dyn Any,
        parent: &'a mut Context<'_>,
    ) -> (ChildId, Context<'a>) {
        let state = (self.state)(widget);
        let context = WindowInstance::scoped_context(self.id, state.dpi, state.viewport, parent);
        (state.content, context)
    }

    pub(crate) fn dispatch(self, widget: &mut dyn Any, event: &WindowEvent) -> UpdateResult {
        (self.event)(widget, event)
    }

    pub(crate) fn unregister(self, context: &mut Context<'_>) {
        context.window_registry().unregister(self.id);
    }
}
/// Declares an independent native window, with content in the same UI tree.
pub struct Window<V: View> {
    label: Option<String>,
    key: Option<u64>,
    config: WindowConfig,
    view: V,
}

impl<V: View> Window<V> {
    pub fn new(view: V) -> Self {
        Self {
            label: None,
            key: None,
            config: WindowConfig::default(),
            view,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_key(mut self, key: u64) -> Self {
        self.key = Some(key);
        self
    }

    pub fn with_config(mut self, config: WindowConfig) -> Self {
        self.config = config;
        self
    }
}

impl<V: View> View for Window<V> {
    fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    fn key(&self) -> Option<u64> {
        self.key
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        let content = ctx.child(&self.view);
        let id = ctx.register_window::<V>(self.config.clone());
        WindowWidget {
            id,
            view_type: PhantomData,
            content,
            dpi: None,
            viewport: None,
        }
    }
}

/// Owns event metrics. The registry owns OsWindow and UiTree owns content and caches.
pub struct WindowWidget<V: View> {
    id: WindowInstanceId,
    view_type: PhantomData<V>,
    content: ChildId,
    dpi: Option<f64>,
    viewport: Option<[f32; 2]>,
}

impl<V: View> WindowWidget<V> {
    fn handle_window_event(&mut self, event: &WindowEvent) -> UpdateResult {
        match event {
            WindowEvent::ScaleFactorChanged { scale_factor } => {
                let changed = self.dpi != Some(*scale_factor);
                self.dpi = Some(*scale_factor);
                if changed {
                    UpdateResult::LAYOUT | UpdateResult::RENDER
                } else {
                    UpdateResult::empty()
                }
            }
            WindowEvent::Resized { inner_size, .. } => {
                // Backend state may already reflect this event; always invalidate the first notice.
                let changed = self.viewport != Some(*inner_size);
                self.viewport = Some(*inner_size);
                if changed {
                    UpdateResult::LAYOUT | UpdateResult::RENDER
                } else {
                    UpdateResult::empty()
                }
            }
            WindowEvent::Theme(_) => UpdateResult::RENDER,
            _ => UpdateResult::empty(),
        }
    }
}

impl<V: View> Widget for WindowWidget<V> {
    type V = Window<V>;

    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        let content = ctx.child(&view.view);
        let registry = ctx.context().window_registry();
        let instance = registry
            .get_mut(self.id)
            .expect("live Window widget is registered");
        self.content = content;
        instance.update_config(view.config.clone());
        // Descendant drawing changes are not yet summarized by UiTree::update.
        // Schedule a window pass even when the window's own declaration is unchanged.
        instance.request_redraw();
        UpdateResult::empty()
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }

    fn is_inside(&self, _: Bounds, _: Point, _: &mut Context) -> bool {
        false
    }

    fn measure(&self, _: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
        Bounds::default()
    }

    fn layout(&self, _: Bounds, _: Children<'_>, _: &mut Context) {
        // UiTree::layout_window lays out content independently of this zero-size declaration.
    }

    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        Draw::default()
    }
}

/// Declarative window settings. Optional fields use None for "leave unspecified"
/// at creation and "do not update" afterwards; None does not clear an OS setting.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub title: String,
    pub inner_size: Option<Apply<Size>>,
    // Surface configuration is supplied by the application when creating pending windows.
    pub min_inner_size: Option<Apply<Size>>,
    pub max_inner_size: Option<Apply<Size>>,
    pub resizable: Apply<bool>,
    pub resize_increments: Option<Apply<Size>>,
    pub position: Option<Apply<Position>>,

    pub maximized: Apply<bool>,
    pub fullscreen: Option<Apply<Fullscreen>>,

    pub visible: Apply<bool>,
    /// Currently creation-only: matcha-window has no transparency setter yet.
    pub transparent: Apply<bool>,
    pub decorations: Apply<bool>,
    pub preferred_theme: Option<Apply<Theme>>,

    pub enabled_buttons: Apply<WindowButtons>,
    /// Currently creation-only: matcha-window has no activation setter yet.
    pub active: Apply<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Apply<T> {
    /// Apply on creation and on every widget update, even if the value is unchanged.
    Always(T),
    /// Use only when the native window is first created.
    Init(T),
}

impl<T> Apply<T> {
    pub fn into_value(self) -> T {
        match self {
            Self::Always(value) | Self::Init(value) => value,
        }
    }
}

impl<T: Copy> Apply<T> {
    fn update_value(self) -> Option<T> {
        match self {
            Self::Always(value) => Some(value),
            Self::Init(_) => None,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Matcha Window".into(),
            inner_size: None,
            min_inner_size: None,
            max_inner_size: None,
            resizable: Apply::Always(true),
            resize_increments: None,
            position: None,
            maximized: Apply::Init(false),
            fullscreen: None,
            visible: Apply::Always(true),
            transparent: Apply::Init(false),
            decorations: Apply::Always(true),
            preferred_theme: None,
            enabled_buttons: Apply::Always(WindowButtons::default()),
            active: Apply::Init(true),
        }
    }
}

impl WindowConfig {
    pub fn into_inner_config(
        self,
        surface_config: wgpu::SurfaceConfiguration,
    ) -> matcha_window::window::WindowConfig {
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        return matcha_window::window::WindowConfig {
            title: self.title,
            inner_size: self.inner_size.map(Apply::into_value),
            surface_config,
            canvas_id: None,
        };

        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        matcha_window::window::WindowConfig {
            title: self.title,
            inner_size: self.inner_size.map(Apply::into_value),
            surface_config,
            min_inner_size: self.min_inner_size.map(Apply::into_value),
            max_inner_size: self.max_inner_size.map(Apply::into_value),
            position: self.position.map(Apply::into_value),
            resizable: self.resizable.into_value(),
            enabled_buttons: self.enabled_buttons.into_value(),
            maximized: self.maximized.into_value(),
            fullscreen: self.fullscreen.map(Apply::into_value),
            visible: self.visible.into_value(),
            transparent: self.transparent.into_value(),
            decorations: self.decorations.into_value(),
            preferred_theme: self.preferred_theme.map(Apply::into_value),
            resize_increments: self.resize_increments.map(Apply::into_value),
            active: self.active.into_value(),
        }
    }

    fn apply_updates(&self, window: &OsWindow) {
        window.set_title(&self.title);
        if let Some(size) = self.inner_size.and_then(Apply::update_value) {
            let [width, height] = physical_size(size, window.dpi());
            window.request_inner_size(width, height);
        }
        if let Some(value) = self.min_inner_size.and_then(Apply::update_value) {
            window.set_min_inner_size(Some(value));
        }
        if let Some(value) = self.max_inner_size.and_then(Apply::update_value) {
            window.set_max_inner_size(Some(value));
        }
        if let Some(value) = self.resize_increments.and_then(Apply::update_value) {
            window.set_resize_increments(Some(value));
        }
        if let Some(value) = self.position.and_then(Apply::update_value) {
            window.set_outer_position(value);
        }
        if let Some(value) = self.resizable.update_value() {
            window.set_resizable(value);
        }
        if let Some(value) = self.maximized.update_value() {
            window.set_maximized(value);
        }
        if let Some(value) = self.fullscreen.and_then(Apply::update_value) {
            window.set_fullscreen(Some(value));
        }
        if let Some(value) = self.visible.update_value() {
            window.set_visible(value);
        }
        if let Some(value) = self.decorations.update_value() {
            window.set_decorations(value);
        }
        if let Some(value) = self.preferred_theme.and_then(Apply::update_value) {
            window.set_theme(Some(value));
        }
        if let Some(value) = self.enabled_buttons.update_value() {
            window.set_enabled_buttons(value);
        }
        // TODO(matcha-window): transparent/active updates need backend setters.
        // Both fields are still passed through at native creation time.
    }
}

fn physical_size(size: Size, dpi: f64) -> [u32; 2] {
    match size {
        Size::Physical { width, height } => [width, height],
        Size::Logical { width, height } => {
            [(width * dpi).round() as u32, (height * dpi).round() as u32]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_size_uses_live_dpi_while_physical_size_is_unchanged() {
        assert_eq!(
            physical_size(
                Size::Logical {
                    width: 101.0,
                    height: 50.0
                },
                1.5
            ),
            [152, 75]
        );
        assert_eq!(physical_size([101, 50].into(), 1.5), [101, 50]);
    }

    #[test]
    fn creation_unwraps_both_policies_and_preserves_application_surface_settings() {
        let mut surface = matcha_window::window::WindowConfig::default().surface_config;
        surface.width = 913;
        surface.height = 717;
        surface.desired_maximum_frame_latency = 2;
        let config = WindowConfig {
            title: "test".into(),
            inner_size: Some(Apply::Init([500, 400].into())),
            resizable: Apply::Always(false),
            maximized: Apply::Init(true),
            ..WindowConfig::default()
        }
        .into_inner_config(surface.clone());
        assert_eq!(config.title, "test");
        assert_eq!(config.inner_size, Some([500, 400].into()));
        assert_eq!(config.surface_config, surface);
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        {
            assert!(!config.resizable);
            assert!(config.maximized);
        }
    }
}
