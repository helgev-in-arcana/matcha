/// Represents the state of an element (e.g., a key or button).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementState {
    /// The element is pressed, with an associated count (e.g., repeat count, click combo).
    Pressed(u32),
    /// The element has been long-pressed, with an associated count.
    LongPressed(u32),
    /// The element has been released, with an associated count.
    Released(u32),
}
