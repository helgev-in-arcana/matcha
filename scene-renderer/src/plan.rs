//! CPU first-use scheduling. A phase owns its snapshot; resource retention and
//! culling cannot postpone generation to a later phase. No GPU placement lives here.
use std::collections::HashSet;

use render_interface::{MaskId, MeshId, Scene, TextureId};

use crate::SceneError;

#[derive(Default)]
pub(crate) struct PhasePlan {
    pub(crate) meshes: Vec<MeshId>,
    pub(crate) textures: Vec<TextureId>,
    pub(crate) masks: Vec<MaskId>,
}

pub(crate) struct FramePlan {
    pub(crate) phases: Vec<PhasePlan>,
}

impl FramePlan {
    pub(crate) fn build(scene: &Scene) -> Result<Self, SceneError> {
        // Check topology first, so following a parent is finite and in bounds.
        for (index, mask) in scene.pixel_masks.iter().enumerate() {
            if mask.parent.is_some_and(|p| p.0 as usize >= index) {
                return Err(SceneError::Invalid("mask parent must precede child".into()));
            }
        }
        let mut meshes = HashSet::new();
        let mut textures = HashSet::new();
        let mut masks = HashSet::new();
        let mut visited_masks = HashSet::new();
        let mut phases = Vec::with_capacity(scene.phases.len());
        for phase in &scene.phases {
            let mut plan = PhasePlan::default();
            for object in &phase.objects {
                if meshes.insert(object.mesh) {
                    plan.meshes.push(object.mesh);
                }
                if textures.insert(object.texture) {
                    plan.textures.push(object.texture);
                }
                let mut index = object.mask;
                while let Some(current) = index {
                    let mask = scene.pixel_masks.get(current.0 as usize).ok_or_else(|| {
                        SceneError::Invalid("object mask index out of bounds".into())
                    })?;
                    if !visited_masks.insert(current) {
                        break;
                    }
                    if meshes.insert(mask.mesh) {
                        plan.meshes.push(mask.mesh);
                    }
                    if masks.insert(mask.texture) {
                        plan.masks.push(mask.texture);
                    }
                    index = mask.parent;
                }
            }
            phases.push(plan);
        }
        Ok(Self { phases })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use render_interface::{
        Matrix4, Object, Phase, PixelMask, PixelMaskIndex, TextureDescriptor, TextureSource,
    };

    fn object(mesh: MeshId, texture: TextureId, mask: Option<u32>) -> Object {
        Object {
            mask: mask.map(PixelMaskIndex),
            ..Object::new(mesh, texture, Matrix4::identity())
        }
    }

    fn mask(mesh: MeshId, texture: MaskId, parent: Option<u32>) -> PixelMask {
        PixelMask {
            mesh,
            texture,
            transform: Matrix4::identity(),
            parent: parent.map(PixelMaskIndex),
        }
    }

    #[test]
    fn ancestor_dependencies_are_scheduled_once_in_the_earliest_referencing_phase() {
        // Planning uses IDs and topology; definition/device validation is a
        // separate stage. No GPU or executable source is needed for this graph.
        let draw_mesh = MeshId::new();
        let root_mesh = MeshId::new();
        let child_mesh = MeshId::new();
        let texture = TextureId::new();
        let root_coverage = MaskId::new();
        let child_coverage = MaskId::new();
        let sibling_coverage = MaskId::new();
        let scene = Scene {
            pixel_masks: vec![
                mask(root_mesh, root_coverage, None),
                mask(child_mesh, child_coverage, Some(0)),
                mask(child_mesh, sibling_coverage, Some(0)),
            ],
            phases: vec![
                Phase {
                    objects: vec![
                        object(draw_mesh, texture, Some(1)),
                        object(draw_mesh, texture, Some(1)),
                    ],
                },
                Phase {
                    objects: vec![object(draw_mesh, texture, Some(2))],
                },
                Phase {
                    objects: vec![object(draw_mesh, texture, Some(1))],
                },
            ],
            ..Scene::default()
        };
        let plan = FramePlan::build(&scene).expect("valid acyclic mask graph");
        let first = &plan.phases[0];
        assert_eq!(first.meshes.len(), 3);
        for mesh in [draw_mesh, root_mesh, child_mesh] {
            assert!(first.meshes.contains(&mesh));
        }
        assert_eq!(first.textures, [texture]);
        assert_eq!(first.masks.len(), 2);
        assert!(first.masks.contains(&root_coverage));
        assert!(first.masks.contains(&child_coverage));
        assert!(plan.phases[1].meshes.is_empty());
        assert!(plan.phases[1].textures.is_empty());
        assert_eq!(plan.phases[1].masks, [sibling_coverage]);
        assert!(plan.phases[2].meshes.is_empty());
        assert!(plan.phases[2].textures.is_empty());
        assert!(plan.phases[2].masks.is_empty());
    }

    #[test]
    fn transparent_and_offscreen_objects_do_not_postpone_snapshot_dependencies() {
        let mesh = MeshId::new();
        let texture = TextureId::new();
        let mut hidden = object(mesh, texture, None);
        hidden.opacity = 0.0;
        hidden.transform[(0, 3)] = 1_000_000.0;
        let scene = Scene {
            phases: vec![
                Phase {
                    objects: vec![hidden],
                },
                Phase {
                    objects: vec![object(mesh, texture, None)],
                },
            ],
            ..Scene::default()
        };
        let plan =
            FramePlan::build(&scene).expect("visibility does not affect resource scheduling");
        assert_eq!(plan.phases[0].meshes, [mesh]);
        assert_eq!(plan.phases[0].textures, [texture]);
        assert!(plan.phases[1].meshes.is_empty());
        assert!(plan.phases[1].textures.is_empty());
    }

    #[test]
    fn unused_definitions_and_empty_phases_request_no_preparation() {
        let mut scene = Scene::default();
        scene
            .resources
            .insert_texture(TextureSource::new(
                TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
                |_| panic!("planning must never invoke resource preparation"),
            ))
            .expect("fresh unused definition");
        scene.phases = vec![Phase::default(), Phase::default()];
        let plan = FramePlan::build(&scene).expect("empty phases are valid");
        assert_eq!(plan.phases.len(), 2);
        assert!(plan.phases.iter().all(|phase| {
            phase.meshes.is_empty() && phase.textures.is_empty() && phase.masks.is_empty()
        }));
        assert!(
            FramePlan::build(&Scene::default())
                .expect("empty scene is valid")
                .phases
                .is_empty()
        );
    }

    #[test]
    fn invalid_mask_links_are_rejected_before_parent_traversal() {
        let mesh = MeshId::new();
        let texture = TextureId::new();
        let coverage = MaskId::new();
        for parent in [0, 1, u32::MAX] {
            let scene = Scene {
                pixel_masks: vec![mask(mesh, coverage, Some(parent))],
                phases: vec![Phase {
                    objects: vec![object(mesh, texture, Some(0))],
                }],
                ..Scene::default()
            };
            assert!(FramePlan::build(&scene).is_err());
        }
        let scene = Scene {
            phases: vec![Phase {
                objects: vec![object(mesh, texture, Some(0))],
            }],
            ..Scene::default()
        };
        assert!(FramePlan::build(&scene).is_err());
    }
}
