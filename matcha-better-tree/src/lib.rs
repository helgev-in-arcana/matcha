//! Retained UI tree with reconciliation, layout, input, and renderer-independent scenes.
//! [`App`] drives the root model and its windows through the matcha-window event loop.
pub mod app;
mod arena;
pub mod component;
pub mod metrics;
pub mod ui_tree;

pub use app::{App, Model, UpdateView};
pub use render_interface;
pub use ui_tree::UiTree;
