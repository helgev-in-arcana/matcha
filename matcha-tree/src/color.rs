#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Color {
    Rgb8USrgb { r: u8, g: u8, b: u8 },
    Rgba8USrgb { r: u8, g: u8, b: u8, a: u8 },
    RgbF32 { r: f32, g: f32, b: f32 },
    RgbaF32 { r: f32, g: f32, b: f32, a: f32 },
    RgbF64 { r: f64, g: f64, b: f64 },
    RgbaF64 { r: f64, g: f64, b: f64, a: f64 },
}

impl Default for Color {
    fn default() -> Self {
        Color::Rgba8USrgb {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        }
    }
}

// a macro to convert integer to float
macro_rules! convert_linear {
    ($x:expr, $type:ty) => {
        if *$x as $type / 255.0 <= 0.04045 {
            *$x as $type / 255.0 / 12.92
        } else {
            ((*$x as $type / 255.0 + 0.055) / 1.055).powf(2.4)
        }
    };
}

macro_rules! convert_srgb_u8 {
    ($x:expr) => {
        if *$x <= 0.0031308 {
            (*$x * 12.92 * 255.0).round() as u8
        } else {
            ((1.055 * $x.powf(-2.4) - 0.055) * 255.0).round() as u8
        }
    };
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color::Rgb8USrgb { r, g, b }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Self {
        Color::Rgba8USrgb {
            r,
            g,
            b,
            a: a as u8,
        }
    }
}

impl Color {
    pub const TRANSPARENT: Color = Color::RgbaF32 {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    pub fn is_transparent(&self) -> bool {
        match self {
            Color::Rgba8USrgb { a, .. } => *a == 0,
            Color::RgbaF32 { a, .. } => *a <= 0.0,
            Color::RgbaF64 { a, .. } => *a <= 0.0,
            _ => false,
        }
    }

    pub fn is_opaque(&self) -> bool {
        match self {
            Color::Rgba8USrgb { a, .. } => *a == 255,
            Color::RgbaF32 { a, .. } => *a >= 1.0,
            Color::RgbaF64 { a, .. } => *a >= 1.0,
            _ => true,
        }
    }

    pub fn to_rgba_u8(&self) -> [u8; 4] {
        match self {
            Color::Rgb8USrgb { r, g, b } => [*r, *g, *b, 255],
            Color::Rgba8USrgb { r, g, b, a } => [*r, *g, *b, *a],
            Color::RgbF32 { r, g, b } => [
                convert_srgb_u8!(r),
                convert_srgb_u8!(g),
                convert_srgb_u8!(b),
                255,
            ],
            Color::RgbaF32 { r, g, b, a } => [
                convert_srgb_u8!(r),
                convert_srgb_u8!(g),
                convert_srgb_u8!(b),
                (a * 255.0).round() as u8,
            ],
            Color::RgbF64 { r, g, b } => [
                convert_srgb_u8!(r),
                convert_srgb_u8!(g),
                convert_srgb_u8!(b),
                255,
            ],
            Color::RgbaF64 { r, g, b, a } => [
                convert_srgb_u8!(r),
                convert_srgb_u8!(g),
                convert_srgb_u8!(b),
                (a * 255.0).round() as u8,
            ],
        }
    }

    pub fn to_rgba_f32(&self) -> [f32; 4] {
        match self {
            Color::Rgb8USrgb { r, g, b } => [
                convert_linear!(r, f32),
                convert_linear!(g, f32),
                convert_linear!(b, f32),
                1.0,
            ],
            Color::Rgba8USrgb { r, g, b, a } => [
                convert_linear!(r, f32),
                convert_linear!(g, f32),
                convert_linear!(b, f32),
                *a as f32 / 255.0,
            ],
            Color::RgbF32 { r, g, b } => [*r, *g, *b, 1.0],
            Color::RgbaF32 { r, g, b, a } => [*r, *g, *b, *a],
            Color::RgbF64 { r, g, b } => [*r as f32, *g as f32, *b as f32, 1.0],
            Color::RgbaF64 { r, g, b, a } => [*r as f32, *g as f32, *b as f32, *a as f32],
        }
    }

    pub fn to_rgba_f64(&self) -> [f64; 4] {
        match self {
            Color::Rgb8USrgb { r, g, b } => [
                convert_linear!(r, f64),
                convert_linear!(g, f64),
                convert_linear!(b, f64),
                1.0,
            ],
            Color::Rgba8USrgb { r, g, b, a } => [
                convert_linear!(r, f64),
                convert_linear!(g, f64),
                convert_linear!(b, f64),
                *a as f64 / 255.0,
            ],
            Color::RgbF32 { r, g, b } => [*r as f64, *g as f64, *b as f64, 1.0],
            Color::RgbaF32 { r, g, b, a } => [*r as f64, *g as f64, *b as f64, *a as f64],
            Color::RgbF64 { r, g, b } => [*r, *g, *b, 1.0],
            Color::RgbaF64 { r, g, b, a } => [*r, *g, *b, *a],
        }
    }
}

impl From<Color> for wgpu::Color {
    fn from(color: Color) -> Self {
        let [r, g, b, a] = color.to_rgba_f32();
        wgpu::Color {
            r: r as f64,
            g: g as f64,
            b: b as f64,
            a: a as f64,
        }
    }
}

impl From<[u8; 3]> for Color {
    fn from(x: [u8; 3]) -> Self {
        Color::Rgb8USrgb {
            r: x[0],
            g: x[1],
            b: x[2],
        }
    }
}

impl From<[u8; 4]> for Color {
    fn from(x: [u8; 4]) -> Self {
        Color::Rgba8USrgb {
            r: x[0],
            g: x[1],
            b: x[2],
            a: x[3],
        }
    }
}

impl From<[f32; 3]> for Color {
    fn from(x: [f32; 3]) -> Self {
        Color::RgbF32 {
            r: x[0],
            g: x[1],
            b: x[2],
        }
    }
}

impl From<[f32; 4]> for Color {
    fn from(x: [f32; 4]) -> Self {
        Color::RgbaF32 {
            r: x[0],
            g: x[1],
            b: x[2],
            a: x[3],
        }
    }
}

impl From<[f64; 3]> for Color {
    fn from(x: [f64; 3]) -> Self {
        Color::RgbF64 {
            r: x[0],
            g: x[1],
            b: x[2],
        }
    }
}

impl From<[f64; 4]> for Color {
    fn from(x: [f64; 4]) -> Self {
        Color::RgbaF64 {
            r: x[0],
            g: x[1],
            b: x[2],
            a: x[3],
        }
    }
}

impl Color {
    pub fn to_wgpu_color(&self) -> wgpu::Color {
        let [r, g, b, a] = self.to_rgba_f64();
        wgpu::Color { r, g, b, a }
    }
}
