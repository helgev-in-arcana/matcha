//! Container / Column / Row — layout containers (children via `Scope::node`).

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, world::EntityWorldMut,
};

use matcha_ecs::{components::view::Key, view::Widget};

/// Which layout a container applies. Constant per widget type; carried as a
/// data component so systems can query it.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutKind {
    Container,
    Column,
    Row,
}

/// Spacing between children (used by Column/Row).
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct Gap(pub f32);

/// A neutral container with a single child area.
#[derive(Default)]
pub struct Container {
    key: Key,
}

impl Container {
    pub fn new() -> Self {
        Self { key: Key::Auto }
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }
}

impl Widget for Container {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        LayoutKind::Container
    }

    fn patch(&self, _entity: &mut EntityWorldMut) {
        // LayoutKind is constant for this type; nothing to patch.
    }
}

macro_rules! stack_widget {
    ($name:ident, $kind:expr) => {
        /// A stacking container.
        pub struct $name {
            key: Key,
            gap: f32,
        }

        impl $name {
            pub fn new() -> Self {
                Self {
                    key: Key::Auto,
                    gap: 0.0,
                }
            }

            pub fn gap(mut self, gap: f32) -> Self {
                self.gap = gap;
                self
            }

            pub fn key(mut self, key: impl Into<Key>) -> Self {
                self.key = key.into();
                self
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Widget for $name {
            fn key(&self) -> Key {
                self.key
            }

            fn bundle(&self) -> impl Bundle {
                ($kind, Gap(self.gap))
            }

            fn patch(&self, entity: &mut EntityWorldMut) {
                if let Some(mut gap) = entity.get_mut::<Gap>() {
                    gap.set_if_neq(Gap(self.gap));
                }
            }
        }
    };
}

stack_widget!(Column, LayoutKind::Column);
stack_widget!(Row, LayoutKind::Row);
