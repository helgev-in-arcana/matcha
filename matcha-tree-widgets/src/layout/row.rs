use nalgebra::Matrix4;

use matcha_tree::{
    event::device_event::DeviceEvent,
    ui_tree::{
        context::UiContext,
        metrics::Constraints,
        widget::{View, Widget, WidgetInteractionResult, WidgetPod},
    },
};
use renderer::render_node::RenderNode;

use crate::types::flex::{AlignItems, JustifyContent};
use crate::types::grow_size::GrowSize;
use crate::types::size::{ChildSize, Size};

use super::update_children;

// MARK: View

pub struct Row {
    pub label: Option<String>,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub items: Vec<Box<dyn View>>,
}

impl Row {
    pub fn new() -> Self {
        Self {
            label: None,
            justify_content: JustifyContent::FlexStart {
                gap: GrowSize::Fixed(Size::px(0.0)),
            },
            align_items: AlignItems::Start,
            items: Vec::new(),
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn justify_content(mut self, jc: JustifyContent) -> Self {
        self.justify_content = jc;
        self
    }

    pub fn align_items(mut self, ai: AlignItems) -> Self {
        self.align_items = ai;
        self
    }

    pub fn push(mut self, item: impl View + 'static) -> Self {
        self.items.push(Box::new(item));
        self
    }
}

impl View for Row {
    fn build(&self, ctx: &UiContext) -> WidgetPod {
        let mut children = Vec::new();
        for item in &self.items {
            children.push(item.build(ctx));
        }
        let mut pod = WidgetPod::new(
            0usize,
            RowWidget {
                justify_content: self.justify_content.clone(),
                align_items: self.align_items,
                children,
            },
        );
        if let Some(label) = &self.label {
            pod = pod.with_label(label.clone());
        }
        pod
    }
}

// MARK: Widget

pub struct RowWidget {
    justify_content: JustifyContent,
    align_items: AlignItems,
    children: Vec<WidgetPod>,
}

impl RowWidget {
    fn calc_gap_and_offset(
        &self,
        container_size: f32,
        total_child_width: f32,
        child_max_height: f32,
        child_count: usize,
        ctx: &UiContext,
    ) -> (f32, f32) {
        if child_count == 0 {
            return (0.0, 0.0);
        }

        let mut rep_child_size = ChildSize::with_size([total_child_width, child_max_height]);

        let (mut gap, mut offset) = match &self.justify_content {
            JustifyContent::FlexStart { gap: g }
            | JustifyContent::FlexEnd { gap: g }
            | JustifyContent::Center { gap: g } => match g {
                GrowSize::Grow(s) => {
                    if child_count >= 2 {
                        let available = container_size - total_child_width;
                        let grow_val =
                            s.size([container_size, child_max_height], &mut rep_child_size, ctx);
                        (
                            (available / (child_count - 1) as f32 * grow_val).max(0.0),
                            0.0,
                        )
                    } else {
                        let offset = match &self.justify_content {
                            JustifyContent::FlexEnd { .. } => container_size - total_child_width,
                            JustifyContent::Center { .. } => {
                                (container_size - total_child_width) / 2.0
                            }
                            _ => 0.0,
                        };
                        (0.0, offset)
                    }
                }
                GrowSize::Fixed(s) => (
                    s.size([container_size, child_max_height], &mut rep_child_size, ctx),
                    0.0,
                ),
            },
            JustifyContent::SpaceAround => {
                let available = container_size - total_child_width;
                let gap = available / child_count as f32;
                (gap, gap / 2.0)
            }
            JustifyContent::SpaceEvenly => {
                let available = container_size - total_child_width;
                let gap = available / (child_count + 1) as f32;
                (gap, gap)
            }
            JustifyContent::SpaceBetween => {
                let available = container_size - total_child_width;
                if child_count >= 2 {
                    ((available / (child_count - 1) as f32).max(0.0), 0.0)
                } else {
                    (0.0, 0.0)
                }
            }
        };

        gap = gap.max(0.0);

        offset = match &self.justify_content {
            JustifyContent::FlexEnd { .. } => {
                container_size - total_child_width - gap * (child_count - 1) as f32
            }
            JustifyContent::Center { .. } => {
                (container_size - total_child_width - gap * (child_count - 1) as f32) / 2.0
            }
            JustifyContent::SpaceAround => gap / 2.0,
            JustifyContent::SpaceEvenly => gap,
            _ => offset,
        };

        (gap, offset)
    }

    fn compute_arrangements(
        &self,
        bounds: [f32; 2],
        ctx: &UiContext,
    ) -> Vec<([f32; 2], Matrix4<f32>)> {
        if self.children.is_empty() {
            return Vec::new();
        }

        let child_constraints = Constraints::new([0.0, bounds[0]], [0.0, bounds[1]]);
        let child_sizes: Vec<[f32; 2]> = self
            .children
            .iter()
            .map(|c| c.measure(&child_constraints, ctx))
            .collect();

        let total_child_width: f32 = child_sizes.iter().map(|s| s[0]).sum();
        let child_max_height: f32 = child_sizes.iter().map(|s| s[1]).fold(0.0_f32, f32::max);

        let (gap, offset) = self.calc_gap_and_offset(
            bounds[0],
            total_child_width,
            child_max_height,
            child_sizes.len(),
            ctx,
        );

        let mut x = offset;
        let mut arrangements = Vec::with_capacity(child_sizes.len());

        for &child_size in &child_sizes {
            let y = match self.align_items {
                AlignItems::Start => 0.0,
                AlignItems::End => (bounds[1] - child_size[1]).max(0.0),
                AlignItems::Center => ((bounds[1] - child_size[1]) / 2.0).max(0.0),
            };
            let affine = Matrix4::new_translation(&nalgebra::Vector3::new(x, y, 0.0));
            arrangements.push((child_size, affine));
            x += child_size[0] + gap;
        }

        arrangements
    }
}

impl Widget for RowWidget {
    type View = Row;

    fn update(&mut self, view: &Row, ctx: &UiContext) -> WidgetInteractionResult {
        self.justify_content = view.justify_content.clone();
        self.align_items = view.align_items;
        update_children(&mut self.children, &view.items, ctx)
    }

    fn device_input(
        &mut self,
        bounds: [f32; 2],
        event: &DeviceEvent,
        ctx: &UiContext,
    ) -> WidgetInteractionResult {
        let arrangements = self.compute_arrangements(bounds, ctx);
        let mut result = WidgetInteractionResult::NoChange;
        for (child, (child_size, affine)) in self.children.iter_mut().zip(arrangements.iter()).rev()
        {
            let child_event = event.transform(*affine);
            match child.device_input(*child_size, &child_event, ctx) {
                WidgetInteractionResult::LayoutNeeded => {
                    result = WidgetInteractionResult::LayoutNeeded;
                }
                WidgetInteractionResult::RedrawNeeded => {
                    if !matches!(result, WidgetInteractionResult::LayoutNeeded) {
                        result = WidgetInteractionResult::RedrawNeeded;
                    }
                }
                WidgetInteractionResult::NoChange => {}
            }
        }
        result
    }

    fn measure(&self, constraints: &Constraints, ctx: &UiContext) -> [f32; 2] {
        if self.children.is_empty() {
            return [0.0, 0.0];
        }

        let mut total_width = 0.0f32;
        let mut max_height = 0.0f32;

        for child in &self.children {
            let s = child.measure(constraints, ctx);
            total_width += s[0];
            max_height = max_height.max(s[1]);
        }

        let (gap, _) = self.calc_gap_and_offset(
            constraints.max_width(),
            total_width,
            max_height,
            self.children.len(),
            ctx,
        );

        if self.children.len() > 1 {
            total_width += gap * (self.children.len() - 1) as f32;
        }

        [
            total_width.min(constraints.max_width()),
            max_height.min(constraints.max_height()),
        ]
    }

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode {
        let arrangements = self.compute_arrangements(bounds, ctx);
        let mut render_node = RenderNode::new();
        for (child, (child_size, affine)) in self.children.iter_mut().zip(arrangements.iter()) {
            let child_node = child.render(*child_size, ctx);
            render_node = render_node.add_child(child_node, *affine);
        }
        render_node
    }
}
