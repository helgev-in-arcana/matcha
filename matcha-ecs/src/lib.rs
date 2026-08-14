// The web build leans on the target being single-threaded in a few places that
// cannot be expressed in safe Rust: wgpu's `fragile-send-sync-non-atomic-wasm`
// (enabled for wasm in Cargo.toml), `WindowSurface`'s Send/Sync impls in
// matcha-window, and `ModelHandle`'s in `model.rs`. All of them hold only
// because `wasm32-unknown-unknown` without `atomics` has exactly one thread.
//
// Fail the build rather than silently lose soundness if that stops being true.
#[cfg(all(web, target_feature = "atomics"))]
compile_error!(
    "matcha-ecs's web build assumes a single-threaded wasm target, but `atomics` \
     is enabled. The Send/Sync assumptions in model.rs, matcha-window's \
     window/surface.rs, and wgpu's fragile-send-sync feature are no longer sound."
);

pub mod clip;
pub mod clipboard;
pub mod components;
pub mod focus;
pub mod input;
pub mod keyboard;
pub mod layout;
pub mod model;
pub mod pick;
pub mod pointer;
pub mod render;
pub mod resources;
pub mod systems;
pub mod tab_order;
pub mod task;
pub mod traversal;
pub mod ui_ecs;
pub mod view;
