pub struct ArenaTree<T> {
    nodes: Vec<Slot<T>>,
    first_free: usize,
}

enum Slot<T> {
    Free { next: usize },
    Used { data: T },
}

impl<T> ArenaTree<T> {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            first_free: 0,
        }
    }

    pub fn alloc(&mut self, data: T) -> usize {
        let alloc_index = self.first_free;
        self.first_free = match self.nodes[alloc_index] {
            Slot::Free { next } => next,
            Slot::Used { .. } => unreachable!(),
        };
        self.nodes[alloc_index] = Slot::Used { data };
        alloc_index
    }

    pub fn free(&mut self, index: usize) {
        self.nodes[index] = Slot::Free {
            next: self.first_free,
        };
        self.first_free = index;
    }
}
