use crate::buffer::Buffer;
use matcha_tree::event::device_event::DeviceEvent;
use matcha_tree::ui_tree::{
    context::UiContext,
    metrics::Constraints,
    widget::{View, Widget, WidgetInteractionResult, WidgetPod},
};
use scene_builder::Draw;
use std::sync::Arc;

use crate::style::Style as _;
use crate::{style, types::size::Size};

// MARK: View

pub struct Image {
    label: Option<String>,
    image_style: style::image::Image,
}

impl Image {
    pub fn new(image: impl Into<style::image::ImageSource>) -> Self {
        Self {
            label: None,
            image_style: style::image::Image::new(image),
        }
    }

    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(label.to_string());
        self
    }

    pub fn size(mut self, size: [Size; 2]) -> Self {
        self.image_style = self.image_style.size(size);
        self
    }
}

impl View for Image {
    fn build(&self, _ctx: &UiContext) -> WidgetPod {
        let mut pod = WidgetPod::new(
            0usize,
            ImageWidget {
                image_style: self.image_style.clone(),
                buffer: Buffer::new(vec![Arc::new(self.image_style.clone())]),
            },
        );
        if let Some(label) = &self.label {
            pod = pod.with_label(label.clone());
        }
        pod
    }
}

// MARK: Widget

pub struct ImageWidget {
    buffer: Buffer,
    image_style: style::image::Image,
}

impl Widget for ImageWidget {
    type View = Image;

    fn update(&mut self, view: &Image, _ctx: &UiContext) -> WidgetInteractionResult {
        if self.image_style != view.image_style {
            self.image_style = view.image_style.clone();
            self.buffer = Buffer::new(vec![Arc::new(self.image_style.clone())]);
            WidgetInteractionResult::LayoutNeeded
        } else {
            WidgetInteractionResult::NoChange
        }
    }

    fn device_input(
        &mut self,
        _bounds: [f32; 2],
        _event: &DeviceEvent,
        _ctx: &UiContext,
    ) -> WidgetInteractionResult {
        WidgetInteractionResult::NoChange
    }

    fn measure(&self, constraints: &Constraints, ctx: &UiContext) -> [f32; 2] {
        self.image_style
            .required_region(constraints, ctx)
            .map(|r| r.size())
            .unwrap_or([0.0, 0.0])
    }

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext, draw: &mut Draw<'_>) {
        self.buffer.paint(bounds, ctx, draw);
    }
}
