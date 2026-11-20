//! Windows are ordinary declarations in the application's single UI tree.
//!
//! The registry owns native window instances and records their arena locations and
//! native event routes. Window scopes read metrics from the registered OS window.
//! UiTree's window entry points scope Context to that window; parent rendering and
//! input stop at native window boundaries. App lifecycle scheduling is still separate.

use std::marker::PhantomData;
use std::sync::Arc;

use matcha_window::event::device_event::DeviceEvent;
use matcha_window::window::{Fullscreen, Position, Size, Theme, Window as OsWindow, WindowButtons};

use crate::metrics::{Bounds, Constraints, Point};
use crate::ui_tree::context::Context;
use crate::ui_tree::widget::{Draw, InputResult, UpdateResult, View, Widget};
use crate::ui_tree::{AnyWindow, Children, DeclContext, MountKind, Mounted, NodeBody};

mod registry;
mod tree;
pub use registry::WindowRegistry;

/// Declares an independent native window, with content in the same UI tree.
pub struct Window<V: View> {
    label: Option<String>,
    key: Option<u64>,
    config: WindowConfig,
    on_close_requested: Option<Arc<CloseHandler>>,
    view: V,
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
type CloseHandler = dyn Fn() + Send + Sync;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
type CloseHandler = dyn Fn();

impl<V: View> Window<V> {
    pub fn new(view: V) -> Self {
        Self {
            label: None,
            key: None,
            config: WindowConfig::default(),
            on_close_requested: None,
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

    /// Notify the model when the user asks to close this window. The native
    /// window stays open until the model omits its declaration.
    pub fn on_close_requested(
        mut self,
        callback: impl Fn() + utils::MaybeSendSync + 'static,
    ) -> Self {
        self.on_close_requested = Some(Arc::new(callback));
        self
    }
}

impl<V: View> View for Window<V> {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    fn key(&self) -> Option<u64> {
        self.key
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        ctx.declare_window(self);
        WindowWidget {
            on_close_requested: self.on_close_requested.clone(),
            view_type: PhantomData,
        }
    }
}

/// Handles events for a window declaration. The registry owns the OS window.
pub struct WindowWidget<V: View> {
    on_close_requested: Option<Arc<CloseHandler>>,
    view_type: PhantomData<fn() -> V>,
}

impl<V: View> MountKind<WindowWidget<V>> {
    fn window() -> Self {
        Self {
            wrap: |mounted| NodeBody::Window(Box::new(mounted)),
        }
    }
}

impl<V: View> AnyWindow for Mounted<WindowWidget<V>> {
    fn close_requested(&self) {
        if let Some(callback) = &self.widget.on_close_requested {
            callback();
        }
    }
}

impl<V: View> Widget for WindowWidget<V> {
    type V = Window<V>;

    fn mount_kind() -> MountKind<Self> {
        MountKind::window()
    }

    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        self.on_close_requested.clone_from(&view.on_close_requested);
        ctx.declare_window(view);
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
    pub min_inner_size: Option<Apply<Size>>,
    pub max_inner_size: Option<Apply<Size>>,
    pub resizable: Apply<bool>,
    pub resize_increments: Option<Apply<Size>>,
    pub position: Option<Apply<Position>>,

    pub maximized: Apply<bool>,
    pub fullscreen: Option<Apply<Fullscreen>>,

    pub visible: Apply<bool>,
    pub transparent: bool,
    pub decorations: Apply<bool>,
    pub preferred_theme: Option<Apply<Theme>>,

    pub enabled_buttons: Apply<WindowButtons>,
    pub active: bool,
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
            transparent: false,
            decorations: Apply::Always(true),
            preferred_theme: None,
            enabled_buttons: Apply::Always(WindowButtons::default()),
            active: true,
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
            transparent: self.transparent,
            decorations: self.decorations.into_value(),
            preferred_theme: self.preferred_theme.map(Apply::into_value),
            resize_increments: self.resize_increments.map(Apply::into_value),
            active: self.active,
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
