//! Native clipboard, backed by `arboard`.

use std::sync::Mutex;

/// A handle to the system clipboard.
///
/// Held open rather than opened per operation: on Windows and X11 opening the
/// clipboard is a real syscall that can fail when another process holds it, and
/// paying that on every keystroke-driven copy is both slower and flakier.
///
/// The inner handle is not `Sync`, so it sits behind a `Mutex` — clipboard
/// access is rare enough that contention is not a consideration, and this keeps
/// the type usable from a resource.
pub struct Clipboard {
    inner: Mutex<Option<arboard::Clipboard>>,
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
    /// Open the system clipboard.
    ///
    /// Never fails: a platform that refuses to hand one over (a headless CI
    /// box with no display server) yields a handle whose operations are no-ops,
    /// so a caller never has to branch on "is there a clipboard".
    pub fn new() -> Self {
        let inner = match arboard::Clipboard::new() {
            Ok(clipboard) => Some(clipboard),
            Err(e) => {
                log::warn!("system clipboard unavailable, copy/paste will do nothing: {e}");
                None
            }
        };
        Self {
            inner: Mutex::new(inner),
        }
    }

    /// The clipboard's current contents as text, or `None` if it holds
    /// something else (or nothing).
    pub fn get_text(&self) -> Option<String> {
        let mut guard = self.inner.lock().ok()?;
        let clipboard = guard.as_mut()?;
        match clipboard.get_text() {
            Ok(text) => Some(text),
            Err(arboard::Error::ContentNotAvailable) => None,
            Err(e) => {
                log::warn!("clipboard read failed: {e}");
                None
            }
        }
    }

    /// Replace the clipboard's contents with `text`. Returns whether it stuck.
    pub fn set_text(&self, text: impl Into<String>) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        let Some(clipboard) = guard.as_mut() else {
            return false;
        };
        match clipboard.set_text(text.into()) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("clipboard write failed: {e}");
                false
            }
        }
    }

    /// Whether a real platform clipboard is behind this handle.
    ///
    /// Only worth checking to explain an inert Copy button; the operations are
    /// safe to call either way.
    pub fn is_available(&self) -> bool {
        self.inner.lock().map(|g| g.is_some()).unwrap_or(false)
    }
}
