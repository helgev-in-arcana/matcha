use nalgebra::Matrix4;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Into<Point> for Bounds {
    fn into(self) -> Point {
        Point {
            x: self.x,
            y: self.y,
            z: self.z,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Into<Bounds> for Point {
    fn into(self) -> Bounds {
        Bounds {
            x: self.x,
            y: self.y,
            z: self.z,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    /// Size in the widget's local coordinate system.
    pub bounds: Bounds,
    /// Affine mapping from this widget's local coordinates to its parent's local coordinates.
    /// This is a relative transform, with homogeneous last row `[0, 0, 0, 1]`.
    pub affine: Matrix4<f32>,
}

impl Default for Position {
    fn default() -> Self {
        Self {
            bounds: Bounds::default(),
            affine: Matrix4::identity(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Constraints {
    pub x: [f32; 2],
    pub y: [f32; 2],
    pub z: [f32; 2],
}
