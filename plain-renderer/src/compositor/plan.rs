//! Pure CPU selection of ordered compositor operations.
//!
//! This plan is independent of resource preparation. An object culled here still
//! participates in the separate first-use preparation plan and therefore cannot
//! postpone a snapshot-dependent source to a later phase. Each operation consumes
//! one uniform record; initial-image and final-output operations are added by the
//! frame recorder, not counted here.
//!
//! Mask bounds are accumulated once in parent order. Consecutive objects sharing
//! a mask do not rebuild its ancestry or repeat its matrix projections. The first
//! four intermediate masks retain reusable ancestor coverage; deeper masks use
//! two-slot ping-pong storage. Planning state survives phase boundaries, but never
//! frame boundaries. Vector capacity survives rebuilds, including phase shrinkage.

use render_interface::{MeshDescriptor, MeshId, Scene};

use super::{intersection, mask_slot, pixel_bounds};
use crate::PlainError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrawOp {
    ClearMask {
        slot: usize,
        scissor: [u32; 4],
    },
    Mask {
        /// Index in Scene.pixel_masks.
        index: usize,
        slot: usize,
        /// Parent coverage attachment slot, or the constant white image.
        parent: Option<usize>,
        scissor: [u32; 4],
    },
    Object {
        /// Index in this phase's Object array.
        index: usize,
        /// Coverage attachment slot, rather than a Scene mask index.
        screen_mask: Option<usize>,
        /// Directly sampled mask's index in Scene.pixel_masks.
        local_mask: Option<usize>,
        scissor: [u32; 4],
    },
}

#[derive(Default)]
pub(crate) struct DrawPlan {
    pub(crate) phases: Vec<Vec<DrawOp>>,
    pub(crate) uniform_count: usize,
    spare_phases: Vec<Vec<DrawOp>>,
    chain: Vec<usize>,
    prefix: Vec<usize>,
    mask_bounds: Vec<[u32; 4]>,
}

fn descriptor(scene: &Scene, id: MeshId) -> Result<&MeshDescriptor, PlainError> {
    scene
        .resources
        .mesh(id)
        .map(|source| source.descriptor())
        .ok_or_else(|| PlainError::Invalid("draw plan references a missing mesh definition".into()))
}

impl DrawPlan {
    pub(crate) fn rebuild(
        &mut self,
        scene: &Scene,
        viewport: [f32; 2],
        size: [u32; 2],
    ) -> Result<(), PlainError> {
        for phase in &mut self.phases {
            phase.clear();
        }
        while self.phases.len() > scene.phases.len() {
            self.spare_phases.push(
                self.phases
                    .pop()
                    .expect("there are more retained phases than scene phases"),
            );
        }
        while self.phases.len() < scene.phases.len() {
            self.phases
                .push(self.spare_phases.pop().unwrap_or_default());
        }
        self.uniform_count = 0;
        self.chain.clear();
        self.prefix.clear();
        self.mask_bounds.clear();
        let result = self.rebuild_inner(scene, viewport, size);
        if result.is_err() {
            // A failed rebuild exposes no operations from either the rejected
            // frame or the preceding plan. Vector capacity remains reusable.
            for phase in &mut self.phases {
                phase.clear();
            }
            self.uniform_count = 0;
            self.chain.clear();
            self.prefix.clear();
            self.mask_bounds.clear();
        }
        result
    }

    fn rebuild_inner(
        &mut self,
        scene: &Scene,
        viewport: [f32; 2],
        size: [u32; 2],
    ) -> Result<(), PlainError> {
        if viewport
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.)
            || size.contains(&0)
        {
            return Err(PlainError::Invalid(
                "invalid draw-plan viewport or target extent".into(),
            ));
        }
        for (index, mask) in scene.pixel_masks.iter().enumerate() {
            let own = pixel_bounds(
                descriptor(scene, mask.mesh)?.bounds,
                &mask.transform,
                viewport,
                size,
            );
            let bounds = if let Some(parent) = mask.parent {
                let parent = parent.0 as usize;
                if parent >= index {
                    return Err(PlainError::Invalid("mask parent must precede child".into()));
                }
                intersection(self.mask_bounds[parent], own)
            } else {
                own
            };
            self.mask_bounds.push(bounds);
        }

        let mut last_mask = None;
        let mut active_slot = 0;
        for (phase, operations) in scene.phases.iter().zip(&mut self.phases) {
            for (index, object) in phase.objects.iter().enumerate() {
                let mesh = descriptor(scene, object.mesh)?;
                let object_mask = object.mask.map(|mask| mask.0 as usize);
                let local_mask = if let Some(mask_index) = object_mask {
                    let mask = scene.pixel_masks.get(mask_index).ok_or_else(|| {
                        PlainError::Invalid("object mask index out of bounds".into())
                    })?;
                    (mask.mesh == object.mesh
                        && mask.transform == object.transform
                        && mesh.non_overlapping)
                        .then_some(mask_index)
                } else {
                    None
                };
                let screen_mask = local_mask.map_or(object_mask, |local| {
                    scene.pixel_masks[local]
                        .parent
                        .map(|parent| parent.0 as usize)
                });
                let mut scissor = pixel_bounds(mesh.bounds, &object.transform, viewport, size);
                if let Some(mask_index) = screen_mask {
                    scissor = intersection(scissor, self.mask_bounds[mask_index]);
                }
                if scissor[2] == 0 || scissor[3] == 0 {
                    continue;
                }

                if screen_mask.is_some() && screen_mask != last_mask {
                    self.chain.clear();
                    let mut ancestor = screen_mask;
                    while let Some(mask_index) = ancestor {
                        self.chain.push(mask_index);
                        ancestor = scene.pixel_masks[mask_index]
                            .parent
                            .map(|parent| parent.0 as usize);
                    }
                    self.chain.reverse();
                    let common = self
                        .prefix
                        .iter()
                        .zip(&self.chain)
                        .take_while(|(old, new)| old == new)
                        .count();
                    for (depth, &mask_index) in self.chain.iter().enumerate() {
                        active_slot = mask_slot(depth);
                        if depth < common {
                            continue;
                        }
                        let bounds = self.mask_bounds[mask_index];
                        operations.push(DrawOp::ClearMask {
                            slot: active_slot,
                            scissor: bounds,
                        });
                        operations.push(DrawOp::Mask {
                            index: mask_index,
                            slot: active_slot,
                            parent: depth.checked_sub(1).map(mask_slot),
                            scissor: bounds,
                        });
                    }
                    // Returning to an ancestor only changes which slot we read.
                    // Its deeper cached descendants remain valid until a branch
                    // change actually overwrites one of the persistent slots.
                    if common < self.chain.len().min(4) {
                        self.prefix.clear();
                        self.prefix.extend(self.chain.iter().take(4).copied());
                    }
                }
                last_mask = screen_mask;
                operations.push(DrawOp::Object {
                    index,
                    screen_mask: screen_mask.map(|_| active_slot),
                    local_mask,
                    scissor,
                });
            }
            self.uniform_count = self
                .uniform_count
                .checked_add(operations.len())
                .ok_or_else(|| PlainError::Invalid("draw operation count overflow".into()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use render_interface::{
        MaskId, MaskSource, Matrix4, MeshSource, Object, Phase, PixelMask, PixelMaskIndex,
        TextureDescriptor, TextureId, TextureSource, wgpu,
    };

    fn mesh(scene: &mut Scene, descriptor: MeshDescriptor) -> MeshId {
        scene
            .resources
            .insert_mesh(MeshSource::new(descriptor, |_| {
                panic!("CPU planning must not invoke resource preparation")
            }))
            .expect("fresh mesh")
    }

    fn base() -> (Scene, MeshId, TextureId, MaskId) {
        let mut scene = Scene::default();
        let mesh = mesh(&mut scene, MeshDescriptor::triangles(6, 0));
        let texture = scene
            .resources
            .insert_texture(TextureSource::new(
                TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
                |_| panic!("CPU planning must not prepare textures"),
            ))
            .expect("fresh texture");
        let coverage = scene
            .resources
            .insert_mask(MaskSource::new(
                TextureDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
                |_| panic!("CPU planning must not prepare coverage"),
            ))
            .expect("fresh coverage");
        (scene, mesh, texture, coverage)
    }

    fn mask(mesh: MeshId, texture: MaskId, parent: Option<usize>) -> PixelMask {
        PixelMask {
            mesh,
            texture,
            transform: Matrix4::identity(),
            parent: parent.map(|index| PixelMaskIndex(index as u32)),
        }
    }

    fn object(mesh: MeshId, texture: TextureId, mask: Option<usize>) -> Object {
        Object {
            mask: mask.map(|index| PixelMaskIndex(index as u32)),
            ..Object::new(mesh, texture, Matrix4::identity())
        }
    }

    #[test]
    fn ten_thousand_objects_sharing_a_deep_mask_need_only_actual_uniforms() {
        let (mut scene, mesh, texture, coverage) = base();
        scene.pixel_masks = (0usize..100)
            .map(|index| mask(mesh, coverage, index.checked_sub(1)))
            .collect();
        scene.phases.push(Phase {
            objects: (0..10_000)
                .map(|_| object(mesh, texture, Some(99)))
                .collect(),
        });
        let mut plan = DrawPlan::default();
        plan.rebuild(&scene, [32., 32.], [32, 32])
            .expect("valid scene");
        assert_eq!(plan.uniform_count, 10_200);
        assert_eq!(plan.phases[0].len(), 10_200);
        assert_eq!(
            plan.phases[0]
                .iter()
                .filter(|op| matches!(op, DrawOp::Mask { .. }))
                .count(),
            100
        );
        assert!(matches!(
            plan.phases[0].last(),
            Some(DrawOp::Object {
                index: 9_999,
                screen_mask: Some(5),
                local_mask: None,
                ..
            })
        ));
        let capacity = plan.phases[0].capacity();
        let pointer = plan.phases[0].as_ptr();
        plan.rebuild(&scene, [32., 32.], [32, 32])
            .expect("warm planning");
        assert_eq!(plan.phases[0].capacity(), capacity);
        assert_eq!(plan.phases[0].as_ptr(), pointer);
        assert_eq!(plan.uniform_count, 10_200);
    }

    #[test]
    fn shared_prefix_survives_phase_boundaries_and_unmasked_objects() {
        let (mut scene, mesh, texture, coverage) = base();
        scene.pixel_masks = vec![
            mask(mesh, coverage, None),
            mask(mesh, coverage, Some(0)),
            mask(mesh, coverage, Some(0)),
        ];
        scene.phases = vec![
            Phase {
                objects: vec![object(mesh, texture, Some(1)), object(mesh, texture, None)],
            },
            Phase {
                objects: vec![object(mesh, texture, Some(2))],
            },
        ];
        let mut plan = DrawPlan::default();
        plan.rebuild(&scene, [16., 16.], [16, 16])
            .expect("valid scene");
        assert_eq!(plan.phases[0].len(), 6);
        assert_eq!(plan.phases[1].len(), 3);
        assert_eq!(plan.uniform_count, 9);
        assert!(matches!(
            plan.phases[1][1],
            DrawOp::Mask {
                index: 2,
                slot: 1,
                parent: Some(0),
                ..
            }
        ));
        let second_pointer = plan.phases[1].as_ptr();
        let second = scene.phases.pop().expect("second phase");
        plan.rebuild(&scene, [16., 16.], [16, 16])
            .expect("fewer phases");
        assert_eq!(plan.phases.len(), 1);
        scene.phases.push(second);
        plan.rebuild(&scene, [16., 16.], [16, 16])
            .expect("phase returns");
        assert_eq!(plan.phases[1].as_ptr(), second_pointer);
    }

    #[test]
    fn shorter_ancestor_reads_preserve_deeper_cached_prefix_slots() {
        let (mut scene, mesh, texture, coverage) = base();
        scene.pixel_masks = (0usize..4)
            .map(|index| mask(mesh, coverage, index.checked_sub(1)))
            .collect();
        scene.phases.push(Phase {
            objects: [3, 0, 2, 1, 3]
                .into_iter()
                .map(|index| object(mesh, texture, Some(index)))
                .collect(),
        });
        let mut plan = DrawPlan::default();
        plan.rebuild(&scene, [16., 16.], [16, 16])
            .expect("ancestor reads do not change stored coverage");
        let masks: Vec<_> = plan.phases[0]
            .iter()
            .filter_map(|operation| match operation {
                DrawOp::Mask { index, .. } => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(masks, [0, 1, 2, 3]);
        // Four clear/draw pairs populate the cached prefix. Five object draws
        // reuse those slots; selecting an ancestor requires no mask redraw.
        assert_eq!(plan.uniform_count, 13);
        let slots: Vec<_> = plan.phases[0]
            .iter()
            .filter_map(|operation| match operation {
                DrawOp::Object { screen_mask, .. } => *screen_mask,
                _ => None,
            })
            .collect();
        assert_eq!(slots, [3, 0, 2, 1, 3]);
    }

    #[test]
    fn another_branch_invalidates_descendants_even_after_short_ancestor_reads() {
        let (mut scene, mesh, texture, coverage) = base();
        scene.pixel_masks = vec![
            mask(mesh, coverage, None),
            mask(mesh, coverage, Some(0)),
            mask(mesh, coverage, Some(1)),
            mask(mesh, coverage, Some(0)),
        ];
        scene.phases.push(Phase {
            objects: [2, 0, 3, 0, 2]
                .into_iter()
                .map(|index| object(mesh, texture, Some(index)))
                .collect(),
        });
        let mut plan = DrawPlan::default();
        plan.rebuild(&scene, [16., 16.], [16, 16])
            .expect("branch overwrite invalidates the old dependent prefix");
        let masks: Vec<_> = plan.phases[0]
            .iter()
            .filter_map(|operation| match operation {
                DrawOp::Mask { index, .. } => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(masks, [0, 1, 2, 3, 1, 2]);
        assert_eq!(plan.uniform_count, 17);
    }

    #[test]
    fn direct_mask_keeps_only_its_parent_in_screen_space() {
        let (mut scene, mesh, texture, coverage) = base();
        let mut descriptor = MeshDescriptor::triangles(6, 0);
        descriptor.non_overlapping = true;
        let direct = self::mesh(&mut scene, descriptor);
        scene.pixel_masks = vec![
            mask(mesh, coverage, None),
            mask(direct, coverage, Some(0)),
            mask(direct, coverage, None),
        ];
        scene.phases.push(Phase {
            objects: vec![
                object(direct, texture, Some(1)),
                object(direct, texture, Some(2)),
            ],
        });
        let mut plan = DrawPlan::default();
        plan.rebuild(&scene, [16., 16.], [16, 16])
            .expect("valid direct masks");
        assert_eq!(plan.uniform_count, 4);
        assert!(matches!(
            plan.phases[0][2],
            DrawOp::Object {
                screen_mask: Some(0),
                local_mask: Some(1),
                ..
            }
        ));
        assert!(matches!(
            plan.phases[0][3],
            DrawOp::Object {
                screen_mask: None,
                local_mask: Some(2),
                ..
            }
        ));
    }

    #[test]
    fn culled_objects_emit_no_operations_and_do_not_advance_mask_state() {
        let (mut scene, unbounded, texture, coverage) = base();
        let mut descriptor = MeshDescriptor::triangles(6, 0);
        descriptor.bounds = Some([[0., 0., 0.], [4., 4., 0.]]);
        let bounded = mesh(&mut scene, descriptor);
        let mut outside = Matrix4::identity();
        outside[(0, 3)] = 100.;
        let mut outside_mask = mask(bounded, coverage, None);
        outside_mask.transform = outside;
        scene.pixel_masks = vec![outside_mask];
        let mut hidden = object(bounded, texture, None);
        hidden.transform = outside;
        scene.phases.push(Phase {
            objects: vec![
                hidden,
                object(unbounded, texture, Some(0)),
                object(bounded, texture, None),
            ],
        });
        let mut plan = DrawPlan::default();
        plan.rebuild(&scene, [16., 16.], [16, 16])
            .expect("valid bounded scene");
        assert_eq!(plan.uniform_count, 1);
        assert_eq!(
            plan.phases[0],
            [DrawOp::Object {
                index: 2,
                screen_mask: None,
                local_mask: None,
                scissor: [0, 0, 4, 4]
            }]
        );
    }

    #[test]
    fn seeded_branch_changes_keep_every_object_bound_to_its_actual_ancestry() {
        let (mut scene, mesh, texture, coverage) = base();
        let mut seed = 0xf3a1_82de_7b44_269du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for index in 0..80 {
            // Include long paths as well as shallow side branches.
            let parent = if index == 0 {
                None
            } else if index % 3 == 0 {
                Some(next() as usize % index)
            } else {
                Some(index - 1)
            };
            scene.pixel_masks.push(mask(mesh, coverage, parent));
        }
        for _ in 0..4 {
            let objects = (0..250)
                .map(|_| {
                    let value = next();
                    object(
                        mesh,
                        texture,
                        (value % 7 != 0).then_some(value as usize % scene.pixel_masks.len()),
                    )
                })
                .collect();
            scene.phases.push(Phase { objects });
        }
        let mut plan = DrawPlan::default();
        plan.rebuild(&scene, [16., 16.], [16, 16])
            .expect("valid forest");
        let mut slots: [Option<Vec<usize>>; 6] = Default::default();
        let mut objects = 0;
        for (phase, operations) in scene.phases.iter().zip(&plan.phases) {
            for operation in operations {
                match *operation {
                    DrawOp::ClearMask { slot, .. } => slots[slot] = None,
                    DrawOp::Mask {
                        index,
                        slot,
                        parent,
                        ..
                    } => {
                        let mut ancestry = parent.map_or_else(Vec::new, |parent| {
                            slots[parent].clone().expect("parent coverage must survive")
                        });
                        ancestry.push(index);
                        slots[slot] = Some(ancestry);
                    }
                    DrawOp::Object {
                        index,
                        screen_mask,
                        local_mask,
                        scissor,
                    } => {
                        assert_eq!(local_mask, None);
                        assert_eq!(scissor, [0, 0, 16, 16]);
                        let mut expected = Vec::new();
                        let mut ancestor = phase.objects[index].mask;
                        while let Some(mask) = ancestor {
                            expected.push(mask.0 as usize);
                            ancestor = scene.pixel_masks[mask.0 as usize].parent;
                        }
                        expected.reverse();
                        let actual = screen_mask.map_or_else(Vec::new, |slot| {
                            slots[slot].clone().expect("screen coverage must survive")
                        });
                        assert_eq!(actual, expected);
                        objects += 1;
                    }
                }
            }
        }
        assert_eq!(objects, 1_000);
        assert_eq!(plan.uniform_count, plan.phases.iter().map(Vec::len).sum());
    }

    #[test]
    fn failed_rebuild_discards_previous_and_partial_operations() {
        let (mut scene, mesh, texture, _) = base();
        scene.phases.push(Phase {
            objects: vec![object(mesh, texture, None)],
        });
        let mut plan = DrawPlan::default();
        plan.rebuild(&scene, [16., 16.], [16, 16])
            .expect("first valid frame");
        scene.phases[0].objects.push(object(mesh, texture, Some(0)));
        assert!(plan.rebuild(&scene, [16., 16.], [16, 16]).is_err());
        assert_eq!(plan.uniform_count, 0);
        assert!(plan.phases.iter().all(Vec::is_empty));
        scene.phases[0].objects.pop();
        plan.rebuild(&scene, [16., 16.], [16, 16])
            .expect("repaired frame");
        assert_eq!(plan.uniform_count, 1);
    }
}
