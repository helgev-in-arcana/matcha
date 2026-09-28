//! Tree UI frontend with framework-owned Scene assembly.
//!
//! Widgets use Draw from render-interface's optional `scene-builder` feature
//! and may reuse owned immutable Sources. Each window owns its Frame and PlainRenderer;
//! texture/mesh placement and GPU residency are private to the backend. Redraw
//! requests do not retain a second widget-local rendering tree.

pub mod color;
pub mod ui_tree;

pub use matcha_window::adapter;
pub use matcha_window::application;
pub use matcha_window::event;
pub use matcha_window::window;
