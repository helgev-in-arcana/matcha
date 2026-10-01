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

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Position {
    pub bounds: Bounds,
    pub affine: Matrix4<f32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Constraints {
    pub x: [f32; 2],
    pub y: [f32; 2],
    pub z: [f32; 2],
}
