//! Tree UI frontend with framework-owned Scene assembly.
//!
//! Widgets emit through scene-builder's Draw on each frame and may reuse owned
//! immutable Source definitions. Each window owns its Frame and SceneRenderer;
//! texture/mesh placement and GPU residency are private to the backend. Redraw
//! requests do not retain a second widget-local rendering tree.

pub mod color;
pub mod ui_tree;

pub use matcha_window::adapter;
pub use matcha_window::application;
pub use matcha_window::event;
pub use matcha_window::window;
