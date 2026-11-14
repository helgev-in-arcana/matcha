//! Live OS windows and their owners in the retained widget tree.
use std::collections::HashMap;

use matcha_window::window::{Window as OsWindow, WindowId};

use crate::arena::Id as UiId;
use crate::ui_tree::context::PlatformState;

/// Removing a window declaration releases its OS window. A suspended window
/// remains registered while its presentation surface is detached.
#[derive(Default)]
pub struct WindowRegistry {
    entries: HashMap<WindowId, WindowEntry>,
}

struct WindowEntry {
    node: UiId,
    window: OsWindow,
    platform: PlatformState,
}

impl WindowRegistry {
    pub(in crate::ui_tree) fn register(&mut self, node: UiId, window: OsWindow) -> WindowId {
        let id = window.id();
        debug_assert!(self.window_for_node(node).is_none());
        self.entries.insert(
            id,
            WindowEntry {
                node,
                window,
                platform: PlatformState::default(),
            },
        );
        id
    }

    pub(super) fn unregister(&mut self, id: WindowId) {
        self.entries.remove(&id);
    }

    pub(in crate::ui_tree) fn remove_node(&mut self, node: UiId) {
        if let Some(id) = self.window_for_node(node).map(OsWindow::id) {
            self.unregister(id);
        }
    }

    /// Reconciliation starts from a declaration node. OS events use the keyed lookup.
    pub(in crate::ui_tree) fn window_for_node(&self, node: UiId) -> Option<&OsWindow> {
        self.entries
            .values()
            .find(|entry| entry.node == node)
            .map(|entry| &entry.window)
    }

    /// Snapshot of the IDs of all registered OS windows.
    pub fn ids(&self) -> Vec<WindowId> {
        self.entries.keys().copied().collect()
    }

    /// Borrows a registered OS window.
    /// ```compile_fail
    /// use matcha_better_tree::ui_tree::window::WindowRegistry;
    /// use matcha_window::window::{Window, WindowId};
    /// fn detach(registry: &WindowRegistry, id: WindowId) -> &'static Window {
    ///     registry.window(id).unwrap()
    /// }
    /// ```
    pub fn window(&self, id: WindowId) -> Option<&OsWindow> {
        self.entries.get(&id).map(|entry| &entry.window)
    }

    pub(crate) fn live_windows_mut(
        &mut self,
    ) -> impl Iterator<Item = (&mut OsWindow, &mut PlatformState)> {
        self.entries
            .values_mut()
            .map(|entry| (&mut entry.window, &mut entry.platform))
    }

    pub(in crate::ui_tree) fn node(&self, id: WindowId) -> Option<UiId> {
        self.entries.get(&id).map(|entry| entry.node)
    }

    pub(in crate::ui_tree) fn platform(&self, id: WindowId) -> &PlatformState {
        &self.entries[&id].platform
    }

    pub(in crate::ui_tree) fn platform_mut(&mut self, id: WindowId) -> &mut PlatformState {
        &mut self
            .entries
            .get_mut(&id)
            .expect("registered window")
            .platform
    }

    /// Flush cursor and IME changes accumulated by widgets in this window scope.
    pub fn sync_to_window(&mut self, id: WindowId) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.platform.sync_to_window(&entry.window);
        }
    }
}
