use crate::style::{PreparedStyle, Style};
use matcha_tree::{
    color::Color,
    ui_tree::{
        context::UiContext,
        metrics::{Constraints, QRect},
    },
};
use parking_lot::Mutex;
/// Clears only the widget's assigned decoration region, never the final Scene.
pub struct ViewportClear {
    pub color: Color,
    cache: Mutex<Option<([u32; 4], PreparedStyle)>>,
}
impl ViewportClear {
    pub fn new(color: Color) -> Self {
        Self {
            color,
            cache: Mutex::new(None),
        }
    }
}
impl Style for ViewportClear {
    fn required_region(&self, c: &Constraints, _ctx: &UiContext) -> Option<QRect> {
        let s = c.max_size();
        (s[0] > 0. && s[1] > 0.).then(|| QRect::new([0., 0.], s))
    }
    fn prepare(&self, _b: [f32; 2], _o: [f32; 2], _ctx: &UiContext) -> Option<PreparedStyle> {
        let color = self.color.to_rgba_f32();
        let key = color.map(f32::to_bits);
        let mut cache = self.cache.lock();
        if cache.as_ref().is_none_or(|(old, _)| *old != key) {
            *cache = Some((
                key,
                PreparedStyle::new(move |mut c| crate::paint::clear(&mut c, color))
                    .with_output_layout(render_interface::PrepareOutputLayout::AnyRegion),
            ));
        }
        cache.as_ref().map(|(_, p)| p.clone())
    }
}
