//! Framework systems.

use bevy_ecs::system::Query;

use crate::components::layout::{GlobalTransform, RectGeometry};

/// Throwaway M1 placement pass: copy each entity's desired [`RectGeometry`]
/// position into its [`GlobalTransform`] as a plain translation.
///
/// This is a stand-in for the real layout pass and is registered in
/// `MatchaSet::Layout`. **Removed wholesale in M3** when `Constraints`/
/// `LayoutOutput` layout arrives.
pub fn temp_place(mut query: Query<(&RectGeometry, &mut GlobalTransform)>) {
    for (geometry, mut transform) in query.iter_mut() {
        transform.affine = nalgebra::Matrix4::new_translation(&nalgebra::Vector3::new(
            geometry.x, geometry.y, 0.0,
        ));
    }
}
