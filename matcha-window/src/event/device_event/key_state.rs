use super::{DeviceEventData, KeyInput};
use std::collections::VecDeque;

pub use keyboard_types::{Code, Key, Modifiers};

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct KeyboardState {
    press_order: VecDeque<(Code, Key)>,
    modifiers: Modifiers,
}

impl KeyboardState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn modifiers_changed(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
    }

    /// Update internal state from `key_input`, fill `key_input.snapshot`, and return
    /// the resulting `DeviceEventData`.
    ///
    /// Returns `None` when the platform could not identify which physical key
    /// was pressed ([`Code::Unidentified`]): there is nothing to track in
    /// `press_order`, so the event is dropped rather than recorded under a key
    /// that would collide with every other unidentified one.
    pub fn keyboard_input(&mut self, key_input: &mut KeyInput) -> Option<DeviceEventData> {
        use super::ElementState;

        let key_code = key_input.physical_key;
        if key_code == Code::Unidentified {
            return None;
        }

        match key_input.state {
            ElementState::Pressed(_) => {
                self.press_order
                    .push_back((key_code, key_input.logical_key.clone()));
            }
            ElementState::Released(_) => {
                if let Some(pos) = self
                    .press_order
                    .iter()
                    .position(|(code, _)| *code == key_code)
                {
                    self.press_order.remove(pos);
                }
            }
            ElementState::LongPressed(_) => {}
        }

        key_input.snapshot = self.clone();

        Some(DeviceEventData::Keyboard(key_input.clone()))
    }
}

impl KeyboardState {
    pub fn is_physical_pressed(&self, key: &Code) -> bool {
        self.press_order.iter().any(|(code, _)| code == key)
    }

    pub fn is_logical_pressed(&self, key: &Key) -> bool {
        self.press_order
            .iter()
            .any(|(_, logical_key)| logical_key == key)
    }

    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    pub fn press_order(&self) -> Vec<(Code, Key)> {
        self.press_order.iter().cloned().collect()
    }
}
