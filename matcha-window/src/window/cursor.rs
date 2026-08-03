//! Cursor shapes, as a backend-neutral enum.
//!
//! Deliberately not a re-export of `winit::window::CursorIcon`: a `baseview`
//! backend has its own set, and the whole point of this crate is that nothing
//! above it names a windowing library. The variants are CSS's `cursor` keywords
//! restricted to the ones every desktop platform actually provides — a widget
//! asking for something exotic would get a silent fallback anyway.

/// What the pointer looks like over a given widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CursorIcon {
    /// The platform's ordinary arrow.
    #[default]
    Default,
    /// A hand — over links and other "this navigates" affordances.
    Pointer,
    /// An I-beam — over selectable or editable text.
    Text,
    /// The busy indicator, with the UI still responsive.
    Progress,
    /// The busy indicator, with the UI blocked.
    Wait,
    /// Crosshair, for precise picking.
    Crosshair,
    /// "This can be moved."
    Move,
    /// "This can be grabbed", and its held counterpart.
    Grab,
    Grabbing,
    /// "This action is not allowed here."
    NotAllowed,
    /// Resize handles, named by the edge or corner they sit on.
    ResizeHorizontal,
    ResizeVertical,
    ResizeNeSw,
    ResizeNwSe,
    /// No cursor at all.
    Hidden,
}

impl CursorIcon {
    /// Whether the platform should draw a cursor at all.
    pub fn is_visible(self) -> bool {
        self != CursorIcon::Hidden
    }
}
