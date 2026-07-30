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

pub mod animation;
pub mod button;
pub mod checkbox;
pub mod color_rect;
pub mod image;
pub mod layout;
pub mod link;
pub mod padding;
pub mod panel;
pub mod rich_text;
pub mod scroll_view;
pub mod shape;
pub mod text;
pub mod text_box;

// Re-exported so downstream code (including this workspace's own tests/demos)
// constructs `RichText`'s style parameter types (`FontWeight`, `FontStyle`,
// `Alignment`, ...) against the exact same `parley` version this crate was
// built against, rather than adding a separate, potentially-mismatched
// dependency.
pub use parley;

pub use animation::Easing;
pub use button::{Button, ButtonLabel};
pub use checkbox::Checkbox;
pub use color_rect::{ColorRect, RectColor};
pub use image::{Image, ImageSource};
pub use layout::{AlignItems, Column, Container, Gap, JustifyContent, LayoutKind, Row};
pub use link::Link;
pub use matcha_ecs::components::input::{Message, OnClick};
pub use padding::{Padding, PaddingLayout};
pub use panel::{Panel, PanelLayout};
pub use rich_text::{RichSpan, RichText, RichTextContent, TextTransform, WhiteSpace};
pub use scroll_view::{
    scroll_view, Axis, Overflow, ScrollState, ScrollView, ScrollViewLayout, ScrollbarStyle,
};
pub use text::{Text, TextContent};
pub use text_box::{TextBox, TextBoxStyle, TextEditor};
