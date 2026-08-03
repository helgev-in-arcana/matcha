use std::hash::Hash;
use std::vec::Vec;

use crate::ui_tree::{
    context::UiContext,
    widget::{View, WidgetInteractionResult, WidgetPod},
};

pub struct SubWidgetsVec<T = ()> {
    vec: Vec<(WidgetPod, T)>,
}

impl<T> SubWidgetsVec<T> {
    pub fn new() -> Self {
        Self { vec: Vec::new() }
    }

    /// Compute the id hash used to match widgets across updates.
    /// Pass the result as the first element of each `new_children` tuple.
    pub fn hash_id(id: impl Hash) -> usize {
        fxhash::hash(&id)
    }

    pub fn len(&self) -> usize {
        self.vec.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vec.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &(WidgetPod, T)> {
        self.vec.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut (WidgetPod, T)> {
        self.vec.iter_mut()
    }
}

impl<T: PartialEq> SubWidgetsVec<T> {
    /// Apply the ID-based diff update algorithm.
    ///
    /// For each entry in `new_children` 窶・a tuple of `(id_hash, view, setting)` 窶・    /// the algorithm tries to match an existing [`WidgetPod`] by `id_hash`.
    /// If a match is found and the view type is compatible the pod is updated in
    /// place; otherwise a new pod is built from the view.
    ///
    /// Returns [`WidgetInteractionResult::LayoutNeeded`] when any child was
    /// added, removed, reordered, or had its setting changed; returns
    /// [`WidgetInteractionResult::NoChange`] when the list is identical.
    pub fn update<'v>(
        &mut self,
        new_children: impl IntoIterator<Item = (usize, &'v dyn View, T)>,
        ctx: &UiContext,
    ) -> WidgetInteractionResult {
        let mut need_rearrange = false;

        // --- Step 1: collect old state ----------------------------------------

        let old_vec = std::mem::take(&mut self.vec);
        let old_ids: Vec<usize> = old_vec.iter().map(|(pod, _)| pod.id_hash()).collect();

        let mut old_map: fxhash::FxHashMap<usize, (WidgetPod, T)> = old_vec
            .into_iter()
            .map(|(pod, setting)| (pod.id_hash(), (pod, setting)))
            .collect();

        // --- Step 2: process new children -------------------------------------

        // Collect once so we can record new_ids for reorder detection.
        // Prefixed with underscore to silence warnings (matches the design
        // note in src-old: reserved for a future O(n) LCS/move-detection pass).
        let new_children: Vec<(usize, &dyn View, T)> = new_children.into_iter().collect();
        let _new_ids: Vec<usize> = new_children.iter().map(|(id, _, _)| *id).collect();

        for (id, view, new_setting) in new_children {
            let mut old_entry = old_map.remove(&id);

            // Try to update the existing pod in place.
            if let Some((pod, _)) = &mut old_entry {
                if pod.try_update(view, ctx).is_err() {
                    // Type mismatch 窶・discard the old pod and build fresh.
                    old_entry = None;
                }
            }

            // Any setting change is treated as layout-affecting (conservative
            // strategy; see design note in src-old about SettingImpact).
            if let Some((_, old_setting)) = &old_entry {
                if *old_setting != new_setting {
                    need_rearrange = true;
                }
            }

            // Reuse the existing pod or build a new one.
            if let Some((pod, _)) = old_entry {
                self.vec.push((pod, new_setting));
            } else {
                let new_pod = view.build(ctx);
                self.vec.push((new_pod, new_setting));
                need_rearrange = true;
            }
        }

        // --- Step 3: detect removals and reordering ---------------------------

        if !old_map.is_empty() {
            need_rearrange = true;
        }

        let new_ids: Vec<usize> = self.vec.iter().map(|(pod, _)| pod.id_hash()).collect();
        if new_ids != old_ids {
            need_rearrange = true;
        }

        // --- Step 4: return result --------------------------------------------

        if need_rearrange {
            WidgetInteractionResult::LayoutNeeded
        } else {
            WidgetInteractionResult::NoChange
        }
    }
}

impl<T> Default for SubWidgetsVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> std::ops::Index<usize> for SubWidgetsVec<T> {
    type Output = (WidgetPod, T);

    fn index(&self, index: usize) -> &Self::Output {
        &self.vec[index]
    }
}

impl<T> std::ops::IndexMut<usize> for SubWidgetsVec<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.vec[index]
    }
}
