use super::{ButtonState, DeviceEventData, MouseInput, MouseLogicalButton};

use web_time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MousePrimaryButton {
    #[default]
    Left,
    Right,
}

#[derive(Debug, Clone, Copy)]
pub struct MouseStateConfig {
    pub combo_duration: Duration,
    pub long_press_duration: Duration,
    pub primary_button: MousePrimaryButton,
    pub pixel_per_line: f32,
}

impl Default for MouseStateConfig {
    /// Sensible defaults:
    /// - Combo window        : 200 ms
    /// - Long-press threshold: 500 ms
    /// - Primary button      : left mouse button
    /// - Scroll pixel/line   : 40 px
    fn default() -> Self {
        Self {
            combo_duration: Duration::from_millis(200),
            long_press_duration: Duration::from_millis(500),
            primary_button: MousePrimaryButton::Left,
            pixel_per_line: 40.0,
        }
    }
}

impl MouseStateConfig {
    pub fn init(self) -> Option<MouseState> {
        let Self {
            combo_duration,
            long_press_duration,
            primary_button,
            pixel_per_line,
        } = self;

        if combo_duration <= long_press_duration {
            Some(MouseState {
                combo_duration,
                long_press_duration,
                position: [0.0, 0.0],
                primary_button,
                pixel_per_line,
                primary: ButtonState::default(),
                dragging_from_primary: None,
                secondary: ButtonState::default(),
                dragging_from_secondary: None,
                middle: ButtonState::default(),
                dragging_from_middle: None,
                back: ButtonState::default(),
                back_dragging_from: None,
                forward: ButtonState::default(),
                forward_dragging_from: None,
            })
        } else {
            None
        }
    }
}

/// Manages the mouse state to detect complex gestures like clicks, drags, and long presses
/// from raw mouse input events.
#[derive(Debug)]
pub struct MouseState {
    /// The maximum duration between clicks to be considered a combo (e.g., double-click).
    combo_duration: Duration,
    /// The duration a button must be held down to be considered a long press.
    long_press_duration: Duration,

    /// The current position of the cursor.
    position: [f32; 2],

    /// The physical button assigned as the primary button.
    primary_button: MousePrimaryButton,

    pixel_per_line: f32,

    // State for each logical button
    primary: ButtonState,
    dragging_from_primary: Option<[f32; 2]>,
    secondary: ButtonState,
    dragging_from_secondary: Option<[f32; 2]>,
    middle: ButtonState,
    dragging_from_middle: Option<[f32; 2]>,
    back: ButtonState,
    back_dragging_from: Option<[f32; 2]>,
    forward: ButtonState,
    forward_dragging_from: Option<[f32; 2]>,
}

impl MouseState {
    pub fn set_primary_button(&mut self, primary_button: MousePrimaryButton) {
        self.primary_button = primary_button;
    }

    pub fn map_logical_button(
        &self,
        button: super::mouse_input::PhysicalMouseButton,
    ) -> Option<MouseLogicalButton> {
        use super::mouse_input::PhysicalMouseButton as W;
        match button {
            W::Left => Some(match self.primary_button {
                MousePrimaryButton::Left => MouseLogicalButton::Primary,
                MousePrimaryButton::Right => MouseLogicalButton::Secondary,
            }),
            W::Right => Some(match self.primary_button {
                MousePrimaryButton::Left => MouseLogicalButton::Secondary,
                MousePrimaryButton::Right => MouseLogicalButton::Primary,
            }),
            W::Middle => Some(MouseLogicalButton::Middle),
            W::Back => Some(MouseLogicalButton::Back),
            W::Forward => Some(MouseLogicalButton::Forward),
            W::Other(_) => None,
        }
    }

    pub fn primary_button(&self) -> MousePrimaryButton {
        self.primary_button
    }

    pub fn set_scroll_pixel_per_line(&mut self, pixel: f32) {
        self.pixel_per_line = pixel;
    }

    pub fn scroll_pixel_per_line(&self) -> f32 {
        self.pixel_per_line
    }

    pub fn pixel_per_line(&self) -> f32 {
        self.pixel_per_line
    }
}

impl MouseState {
    /// Handles a mouse move event.
    ///
    /// `position` is in physical pixels (already converted by the caller).
    pub fn cursor_moved(&mut self, position: [f32; 2]) -> DeviceEventData {
        let prev_position = self.position;
        self.position = position;

        if self.primary.is_pressed() && self.dragging_from_primary.is_none() {
            self.dragging_from_primary = Some(prev_position);
        }
        if self.secondary.is_pressed() && self.dragging_from_secondary.is_none() {
            self.dragging_from_secondary = Some(prev_position);
        }
        if self.middle.is_pressed() && self.dragging_from_middle.is_none() {
            self.dragging_from_middle = Some(prev_position);
        }
        if self.back.is_pressed() && self.back_dragging_from.is_none() {
            self.back_dragging_from = Some(prev_position);
        }
        if self.forward.is_pressed() && self.forward_dragging_from.is_none() {
            self.forward_dragging_from = Some(prev_position);
        }

        Self::new_mouse_event(
            self.dragging_from_primary,
            self.dragging_from_secondary,
            self.dragging_from_middle,
            None,
        )
    }

    /// Generates a `CursorEntered` event.
    pub fn cursor_entered(&self) -> DeviceEventData {
        Self::new_mouse_event(
            self.dragging_from_primary,
            self.dragging_from_secondary,
            self.dragging_from_middle,
            Some(MouseInput::Entered),
        )
    }

    /// Generates a `CursorLeft` event.
    pub fn cursor_left(&self) -> DeviceEventData {
        Self::new_mouse_event(
            self.dragging_from_primary,
            self.dragging_from_secondary,
            self.dragging_from_middle,
            Some(MouseInput::Left),
        )
    }

    /// Generates a `MouseScroll` event.
    ///
    /// `delta` is in physical pixels (LineDelta → pixel conversion done by caller using
    /// `pixel_per_line()`).
    pub fn mouse_wheel(&self, delta: [f32; 2]) -> DeviceEventData {
        Self::new_mouse_event(
            self.dragging_from_primary,
            self.dragging_from_secondary,
            self.dragging_from_middle,
            Some(MouseInput::Scroll { delta }),
        )
    }

    /// Handles a logical mouse button press.
    pub fn button_pressed(&mut self, button: MouseLogicalButton) -> Option<DeviceEventData> {
        let now = Instant::now();
        let combo_duration = self.combo_duration;
        let (button_state, _) = self.get_mut_button_state(button);
        let click_state = button_state.press(now, combo_duration);

        Some(Self::new_mouse_event(
            self.dragging_from_primary,
            self.dragging_from_secondary,
            self.dragging_from_middle,
            Some(MouseInput::Click {
                click_state,
                button,
            }),
        ))
    }

    /// Handles a logical mouse button release.
    pub fn button_released(&mut self, button: MouseLogicalButton) -> Option<DeviceEventData> {
        let (button_state, dragging_from) = self.get_mut_button_state(button);
        let click_state = button_state.release();
        *dragging_from = None;

        Some(Self::new_mouse_event(
            self.dragging_from_primary,
            self.dragging_from_secondary,
            self.dragging_from_middle,
            Some(MouseInput::Click {
                click_state,
                button,
            }),
        ))
    }

    /// Detects long presses for all mouse buttons.
    ///
    /// This method should be called on every frame update.
    pub fn long_pressing_detection(&mut self) -> Vec<DeviceEventData> {
        let now = Instant::now();

        let mut events = Vec::new();
        let buttons = [
            (
                MouseLogicalButton::Primary,
                &mut self.primary,
                self.dragging_from_primary,
            ),
            (
                MouseLogicalButton::Secondary,
                &mut self.secondary,
                self.dragging_from_secondary,
            ),
            (
                MouseLogicalButton::Middle,
                &mut self.middle,
                self.dragging_from_middle,
            ),
            (
                MouseLogicalButton::Back,
                &mut self.back,
                self.back_dragging_from,
            ),
            (
                MouseLogicalButton::Forward,
                &mut self.forward,
                self.forward_dragging_from,
            ),
        ];

        let dragging_primary = self.dragging_from_primary;
        let dragging_secondary = self.dragging_from_secondary;
        let dragging_middle = self.dragging_from_middle;

        for (logical_button, button_state, dragging_from) in buttons {
            if dragging_from.is_none() {
                if let Some(click_state) =
                    button_state.detect_long_press(now, self.long_press_duration)
                {
                    let event = Self::new_mouse_event(
                        dragging_primary,
                        dragging_secondary,
                        dragging_middle,
                        Some(MouseInput::Click {
                            click_state,
                            button: logical_button,
                        }),
                    );
                    events.push(event);
                }
            }
        }
        events
    }
}

// helper methods
impl MouseState {
    pub fn position(&self) -> [f32; 2] {
        self.position
    }

    /// Gets mutable references to the state for a specific logical button.
    fn get_mut_button_state(
        &mut self,
        button: MouseLogicalButton,
    ) -> (&mut ButtonState, &mut Option<[f32; 2]>) {
        match button {
            MouseLogicalButton::Primary => (&mut self.primary, &mut self.dragging_from_primary),
            MouseLogicalButton::Secondary => {
                (&mut self.secondary, &mut self.dragging_from_secondary)
            }
            MouseLogicalButton::Middle => (&mut self.middle, &mut self.dragging_from_middle),
            MouseLogicalButton::Back => (&mut self.back, &mut self.back_dragging_from),
            MouseLogicalButton::Forward => (&mut self.forward, &mut self.forward_dragging_from),
        }
    }

    /// A helper function to create a `DeviceEventData::MouseInput`.
    fn new_mouse_event(
        dragging_from_primary: Option<[f32; 2]>,
        dragging_from_secondary: Option<[f32; 2]>,
        dragging_from_middle: Option<[f32; 2]>,
        event: Option<MouseInput>,
    ) -> DeviceEventData {
        DeviceEventData::MouseInput {
            dragging_from_primary,
            dragging_from_secondary,
            dragging_from_middle,
            event,
        }
    }
}
