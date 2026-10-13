//! Generational storage. Reservations are explicit and must be filled or cancelled by callers.
//! IDs belong to the arena that issued them; they must not be used with another arena.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Id {
    index: usize,
    generation: u64,
}

enum Slot<T> {
    Free {
        next: Option<usize>,
        generation: u64,
    },
    Reserved {
        generation: u64,
    },
    Occupied {
        value: T,
        generation: u64,
    },
}

pub(crate) struct Arena<T> {
    slots: Vec<Slot<T>>,
    first_free: Option<usize>,
}

impl<T> Arena<T> {
    pub(crate) fn new() -> Self {
        Self {
            slots: Vec::new(),
            first_free: None,
        }
    }

    /// Reserve an ID without exposing a value or making the slot available to other reserves.
    /// There is no automatic cancellation on early return or panic.
    pub(crate) fn reserve(&mut self) -> Id {
        if let Some(index) = self.first_free {
            let Slot::Free { next, generation } = self.slots[index] else {
                unreachable!("free list must contain only free slots");
            };
            let generation = generation
                .checked_add(1)
                .expect("arena generation exhausted");
            self.first_free = next;
            self.slots[index] = Slot::Reserved { generation };
            Id { index, generation }
        } else {
            let id = Id {
                index: self.slots.len(),
                generation: 0,
            };
            self.slots.push(Slot::Reserved {
                generation: id.generation,
            });
            id
        }
    }

    /// Fill a current reservation. On a stale ID or wrong slot state, return the value unchanged.
    pub(crate) fn insert_reserved(&mut self, id: Id, value: T) -> Result<(), T> {
        match self.slots.get_mut(id.index) {
            Some(slot @ Slot::Reserved { .. }) => {
                let Slot::Reserved { generation } = slot else {
                    unreachable!()
                };
                if *generation != id.generation {
                    return Err(value);
                }
                *slot = Slot::Occupied {
                    value,
                    generation: id.generation,
                };
                Ok(())
            }
            _ => Err(value),
        }
    }

    /// Release only a current reservation. Never removes an occupied value.
    pub(crate) fn cancel_reserved(&mut self, id: Id) -> bool {
        let Some(slot) = self.slots.get_mut(id.index) else {
            return false;
        };
        if !matches!(slot, Slot::Reserved { generation } if *generation == id.generation) {
            return false;
        }
        *slot = Slot::Free {
            next: self.first_free,
            generation: id.generation,
        };
        self.first_free = Some(id.index);
        true
    }

    pub(crate) fn get(&self, id: Id) -> Option<&T> {
        match self.slots.get(id.index)? {
            Slot::Occupied { value, generation } if *generation == id.generation => Some(value),
            _ => None,
        }
    }

    pub(crate) fn get_mut(&mut self, id: Id) -> Option<&mut T> {
        match self.slots.get_mut(id.index)? {
            Slot::Occupied { value, generation } if *generation == id.generation => Some(value),
            _ => None,
        }
    }

    /// Remove only an occupied value. Reservations must be cancelled explicitly.
    pub(crate) fn remove(&mut self, id: Id) -> Option<T> {
        let slot = self.slots.get_mut(id.index)?;
        if !matches!(slot, Slot::Occupied { generation, .. } if *generation == id.generation) {
            return None;
        }
        let old = std::mem::replace(
            slot,
            Slot::Free {
                next: self.first_free,
                generation: id.generation,
            },
        );
        self.first_free = Some(id.index);
        let Slot::Occupied { value, .. } = old else {
            unreachable!()
        };
        Some(value)
    }

    /// Drop all values and cancel all reservations, preserving generations and storage capacity.
    pub(crate) fn clear(&mut self) {
        let len = self.slots.len();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let generation = match slot {
                Slot::Free { generation, .. }
                | Slot::Reserved { generation }
                | Slot::Occupied { generation, .. } => *generation,
            };
            *slot = Slot::Free {
                next: (index + 1 < len).then_some(index + 1),
                generation,
            };
        }
        self.first_free = (len != 0).then_some(0);
    }

    /// Iterate live values in slot order, excluding free and reserved slots.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (Id, &T)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot {
                Slot::Occupied { value, generation } => Some((
                    Id {
                        index,
                        generation: *generation,
                    },
                    value,
                )),
                _ => None,
            })
    }
}

#[cfg(test)]
mod tests;
