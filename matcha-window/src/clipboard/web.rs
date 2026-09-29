//! Web clipboard: an inert handle.
//!
//! The browser's clipboard is `navigator.clipboard`, which is **asynchronous**
//! and permission-gated — `readText()` returns a Promise and, in most browsers,
//! only resolves inside a user gesture. Neither fits the synchronous
//! `Option<String>` / `bool` shape the rest of the framework calls, so wiring it
//! up is a real design question (a request/response pass through the event loop,
//! most likely) rather than a missing function body.
//!
//! Until then this reports itself unavailable, which is exactly the case the API
//! was already built to express: [`Clipboard::new`] never fails, the operations
//! are always safe to call, and [`Clipboard::is_available`] is there to explain
//! an inert Copy button. A widget needs no `cfg` of its own.

/// A handle to the system clipboard. On the web, always inert — see the module
/// docs.
pub struct Clipboard {
    _private: (),
}

impl std::fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clipboard").finish_non_exhaustive()
    }
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard {
    /// Open the system clipboard. Never fails; on the web the result is inert.
    pub fn new() -> Self {
        log::info!(
            "clipboard is not implemented on the web; copy/paste will do nothing \
             (navigator.clipboard is async and gesture-gated)"
        );
        Self { _private: () }
    }

    /// Always `None` on the web.
    pub fn get_text(&self) -> Option<String> {
        None
    }

    /// Always `false` on the web.
    pub fn set_text(&self, _text: impl Into<String>) -> bool {
        false
    }

    /// Always `false` on the web.
    pub fn is_available(&self) -> bool {
        false
    }
}
