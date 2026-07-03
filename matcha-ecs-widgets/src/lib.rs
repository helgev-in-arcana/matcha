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

pub mod button;
pub mod color_rect;
pub mod layout;
pub mod text;

pub use button::{Button, ButtonLabel, Message, OnClick};
pub use color_rect::{ColorRect, RectColor};
pub use layout::{Column, Container, Gap, LayoutKind, Row};
pub use text::{Text, TextContent};
