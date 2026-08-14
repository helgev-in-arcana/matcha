//! The system clipboard.
//!
//! Lives here rather than in the UI layers for the same reason cursor shapes
//! and IME do: it is a platform capability, and nothing above this crate should
//! have to name the library that provides it.
//!
//! # Why it is a separate delivery path
//!
//! Copy and paste look like keyboard events but are not routed like them. The
//! clipboard is a *side channel* the OS owns: a paste can arrive from a menu
//! item or a middle-click with no key event at all, and what comes back is not
//! necessarily text. So this is an ordinary object a widget reaches for when it
//! handles its own shortcut, not something dispatched down the focus path.
//!
//! # v1 is text only
//!
//! [`Clipboard::get_text`]/[`set_text`](Clipboard::set_text) and nothing else.
//! Images and arbitrary MIME types are a real extension, not an oversight: they
//! need a format-negotiation API (ask what the clipboard *has*, then request
//! one of those), which is a different shape from these two calls and has no
//! consumer yet.
//!
//! # Failures are silent by design
//!
//! Every platform clipboard can fail transiently — another process holds it
//! open, the format is not one we asked for, there is a wayland handshake in
//! flight. None of that is actionable by a widget, and none of it should abort
//! whatever the user was doing, so the API returns `Option`/`bool` and logs.
//!
//! # Platforms
//!
//! Native is backed by `arboard`. The web has no synchronous clipboard API at
//! all, so it gets an inert handle — see `clipboard/web.rs` for why that is a
//! design question rather than a missing function body.

#[cfg(not(web))]
#[path = "clipboard/native.rs"]
mod imp;
#[cfg(web)]
#[path = "clipboard/web.rs"]
mod imp;

pub use imp::Clipboard;
