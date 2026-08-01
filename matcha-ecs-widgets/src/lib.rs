//! Concrete widget implementations for `matcha-ecs`.
//!
//! This crate depends on `matcha-ecs` (the framework core: `Widget` trait,
//! `Scope`, reconcile, framework-shared components) and never the reverse —
//! the core must stay independent of any particular widget implementation.
//!
//! Rendering/layout are not wired yet, so each widget's bundle carries only
//! data components; behaviour will be added by systems later. Each widget is a
//! distinct type (so the reconciler can tell them apart by `TypeId`) exposing a
//! `.key(id)` builder to set its intrinsic reconciliation key.

pub mod anchor;
pub mod animation;
pub mod box_style;
pub mod button;
pub mod checkbox;
pub mod color_rect;
pub mod image;
pub mod interaction;
pub mod layout;
pub mod link;
pub mod padding;
pub mod panel;
pub mod rich_text;
pub mod scroll_view;
pub mod shape;
pub mod slider;
pub mod sizing;
pub mod text;
pub mod text_box;

// Re-exported so downstream code (including this workspace's own tests/demos)
// constructs `RichText`'s style parameter types (`FontWeight`, `FontStyle`,
// `Alignment`, ...) against the exact same `parley` version this crate was
// built against, rather than adding a separate, potentially-mismatched
// dependency.
pub use parley;

pub use anchor::Anchor;
pub use animation::Easing;
pub use box_style::{box_node, BoxShadow, BoxStyle, Corners, Sides};
pub use button::{Button, ButtonLabel};
pub use checkbox::Checkbox;
pub use color_rect::{ColorRect, RectColor};
pub use image::{Image, ImageSource, ObjectFit};
pub use interaction::{ColorCell, InteractionColors};
pub use layout::{AlignItems, Column, Container, Gap, JustifyContent, LayoutKind, Row, Wrap};
pub use link::Link;
pub use matcha_ecs::components::input::{Cursor, Message, OnClick};
pub use matcha_ecs::components::render::ZIndex;
pub use matcha_window::window::CursorIcon;
pub use padding::{Padding, PaddingLayout};
pub use panel::{Panel, PanelLayout};
pub use rich_text::{RichSpan, RichText, RichTextContent, TextTransform, WhiteSpace};
pub use scroll_view::{
    scroll_view, Axis, Overflow, ScrollState, ScrollView, ScrollViewLayout, ScrollbarStyle,
};
pub use sizing::{Length, Sizing};
pub use slider::{Slider, SliderRange};
pub use text::{Text, TextContent};
pub use text_box::{TextBox, TextBoxStyle, TextEditor};

/// Every system this crate's widgets need, ready for
/// [`UiEcs::with_pre_layout_systems`](matcha_ecs::ui_ecs::UiEcs::with_pre_layout_systems).
///
/// One registration instead of three. Each module still exposes its own
/// `default_systems()` for an app that wants only part of it.
///
/// ```ignore
/// UiEcs::new(model, view, reduce)
///     .with_pre_layout_systems(matcha_ecs_widgets::default_systems())
/// ```
///
/// Leaving this out is the one real footgun in this crate: exit fades never
/// despawn, text boxes never re-lay-out, carets never blink, and colour
/// transitions never advance.
pub fn default_systems() -> bevy_ecs::schedule::ScheduleConfigs<bevy_ecs::system::ScheduleSystem> {
    use bevy_ecs::schedule::IntoScheduleConfigs;
    (
        animation::default_systems(),
        interaction::default_systems(),
        text_box::default_systems(),
    )
        .into_configs()
}
