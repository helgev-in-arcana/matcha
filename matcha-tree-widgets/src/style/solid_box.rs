use crate::style::{PreparedStyle, Style};
use matcha_tree::{
    color::Color,
    ui_tree::{
        context::UiContext,
        metrics::{Constraints, QRect},
    },
};
use parking_lot::Mutex;
pub struct SolidBox {
    pub color: Color,
    cache: Mutex<Option<([u32; 8], PreparedStyle)>>,
}
impl SolidBox {
    pub fn new(color: Color) -> Self {
        Self {
            color,
            cache: Mutex::new(None),
        }
    }
}
impl Style for SolidBox {
    fn required_region(&self, constraints: &Constraints, _ctx: &UiContext) -> Option<QRect> {
        let s = constraints.max_size();
        (s[0] > 0. && s[1] > 0.).then(|| QRect::new([0., 0.], s))
    }
    fn prepare(
        &self,
        boundary: [f32; 2],
        offset: [f32; 2],
        _ctx: &UiContext,
    ) -> Option<PreparedStyle> {
        let rgba = self.color.to_rgba_f32();
        let key = [
            boundary[0],
            boundary[1],
            offset[0],
            offset[1],
            rgba[0],
            rgba[1],
            rgba[2],
            rgba[3],
        ]
        .map(f32::to_bits);
        let mut cache = self.cache.lock();
        if cache.as_ref().is_none_or(|(old, _)| *old != key) {
            let vertices = crate::paint::rectangle([0., 0.], boundary, rgba, offset);
            *cache = Some((
                key,
                PreparedStyle::new(move |mut c| crate::paint::draw(&mut c, &vertices, None)),
            ));
        }
        cache.as_ref().map(|(_, p)| p.clone())
    }
}
