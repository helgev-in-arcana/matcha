use super::ElementState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhysicalMouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// Represents a logical mouse button, abstracting away the physical primary/secondary mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseLogicalButton {
    Primary,
    Secondary,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollDelta {
    LineDelta(f32, f32),
    PixelDelta([f32; 2]),
}

/// Represents a specific type of mouse event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MouseInput {
    // Input only
    ButtonInput {
        state: ElementState, // pressed(0) or released(0)
        button: PhysicalMouseButton,
    },
    ScrollRaw {
        delta: ScrollDelta,
    },

    // Used for input tracking and output
    Moved {
        position: [f32; 2],
    },
    Entered,
    Left,

    // Output only (after stateful mapping)
    Click {
        click_state: ElementState,
        button: MouseLogicalButton,
    },
    Scroll {
        delta: [f32; 2],
    },
}
