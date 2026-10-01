use std::any::Any;

pub trait View: Any {
    fn label(&self) -> &str;
    fn id(&self) -> u64;
    fn build(&self) -> impl Widget;
}

pub struct WidgetPod {
    common_data: CommonData,
    widget: Box<dyn Widget>,
}

impl WidgetPod {}

pub struct CommonData {
    label: String,
    id_hash: u64,
    measure_cache: Option<()>,
    layout_cache: Option<()>,
}

pub trait Widget {
    fn update(&mut self);

    fn input(&mut self);

    fn is_inside(&self) -> bool;

    fn measure(&self);

    fn render(&self);
}
