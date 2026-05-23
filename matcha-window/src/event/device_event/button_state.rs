use super::ElementState;
use web_time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub(super) enum ClickStatus {
    #[default]
    Released,
    Pressed,
    LongPressed,
}

/// Manages the state of a single button to detect combos and long presses.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ButtonState {
    /// The current click status of the button.
    pub(super) status: ClickStatus,
    /// The timestamp of the last button press.
    last_clicked_at: Option<Instant>,
    /// The number of consecutive clicks.
    click_combo: u32,
}

impl ButtonState {
    /// Checks if the button is currently pressed (not released).
    pub(super) fn is_pressed(&self) -> bool {
        self.status != ClickStatus::Released
    }

    /// Handles a button press event, updating the combo count and status.
    /// Returns the corresponding `ElementState` (Pressed with combo count).
    pub(super) fn press(&mut self, now: Instant, combo_duration: Duration) -> ElementState {
        if let Some(last_clicked_at) = self.last_clicked_at {
            if now.duration_since(last_clicked_at) <= combo_duration {
                self.click_combo += 1;
            } else {
                self.click_combo = 1;
            }
        } else {
            self.click_combo = 1;
        }

        self.last_clicked_at = Some(now);
        self.status = ClickStatus::Pressed;

        ElementState::Pressed(self.click_combo)
    }

    /// Handles a button release event, resetting the status.
    /// Returns the corresponding `ElementState` (Released with combo count).
    pub(super) fn release(&mut self) -> ElementState {
        self.status = ClickStatus::Released;
        ElementState::Released(self.click_combo)
    }

    /// Detects a long press.
    ///
    /// This method checks if the button has been held down for the `long_press_duration`
    /// and is currently in the `Pressed` state. If a long press is detected, the status
    /// is updated to `LongPressed` and `Some(ElementState::LongPressed)` is returned.
    /// Otherwise, `None` is returned.
    pub(super) fn detect_long_press(
        &mut self,
        now: Instant,
        long_press_duration: Duration,
    ) -> Option<ElementState> {
        if self.status == ClickStatus::Pressed {
            if let Some(last_clicked_at) = self.last_clicked_at {
                if now.duration_since(last_clicked_at) >= long_press_duration {
                    self.status = ClickStatus::LongPressed;
                    return Some(ElementState::LongPressed(self.click_combo));
                }
            }
        }
        None
    }
}
