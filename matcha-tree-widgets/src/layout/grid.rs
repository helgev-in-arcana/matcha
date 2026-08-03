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

use crate::types::{
    grow_size::GrowSize,
    size::{ChildSize, Size},
};

// MARK: View

pub struct GridItem {
    pub item: Box<dyn View>,
    pub column: [usize; 2],
    pub row: [usize; 2],
}

pub struct Grid {
    pub label: Option<String>,
    pub template_columns: Vec<GrowSize>,
    pub template_rows: Vec<GrowSize>,
    pub gap_columns: Size,
    pub gap_rows: Size,
    pub items: Vec<GridItem>,
}

impl Grid {
    pub fn new() -> Self {
        Self {
            label: None,
            template_columns: Vec::new(),
            template_rows: Vec::new(),
            gap_columns: Size::px(0.0),
            gap_rows: Size::px(0.0),
            items: Vec::new(),
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn template_columns(mut self, columns: Vec<GrowSize>) -> Self {
        self.template_columns = columns;
        self
    }

    pub fn template_rows(mut self, rows: Vec<GrowSize>) -> Self {
        self.template_rows = rows;
        self
    }

    pub fn gap_columns(mut self, gap: Size) -> Self {
        self.gap_columns = gap;
        self
    }

    pub fn gap_rows(mut self, gap: Size) -> Self {
        self.gap_rows = gap;
        self
    }

    pub fn item(mut self, item: impl View + 'static, column: [usize; 2], row: [usize; 2]) -> Self {
        self.items.push(GridItem {
            item: Box::new(item),
            column,
            row,
        });
        self
    }
}

impl Default for Grid {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Grid {
    fn build(&self, ctx: &UiContext) -> WidgetPod {
        let children = self
            .items
            .iter()
            .map(|gi| {
                (
                    gi.item.build(ctx),
                    GridChildSetting {
                        column: gi.column,
                        row: gi.row,
                    },
                )
            })
            .collect();

        let mut pod = WidgetPod::new(
            0usize,
            GridWidget {
                template_columns: self.template_columns.clone(),
                template_rows: self.template_rows.clone(),
                gap_columns: self.gap_columns.clone(),
                gap_rows: self.gap_rows.clone(),
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

#[derive(Clone, PartialEq)]
pub struct GridChildSetting {
    pub column: [usize; 2],
    pub row: [usize; 2],
}

pub struct GridWidget {
    template_columns: Vec<GrowSize>,
    template_rows: Vec<GrowSize>,
    gap_columns: Size,
    gap_rows: Size,
    children: Vec<(WidgetPod, GridChildSetting)>,
}

impl GridWidget {
    fn calc_grid_layout(
        &self,
        parent_size: [f32; 2],
        ctx: &UiContext,
    ) -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
        let (col_px_sum, col_grow_sum) =
            self.template_columns
                .iter()
                .fold((0.0f32, 0.0f32), |(sum, grow), size| match size {
                    GrowSize::Fixed(s) => (
                        sum + s.size(parent_size, &mut ChildSize::default(), ctx),
                        grow,
                    ),
                    GrowSize::Grow(s) => (
                        sum,
                        grow + s.size(parent_size, &mut ChildSize::default(), ctx),
                    ),
                });

        let (row_px_sum, row_grow_sum) =
            self.template_rows
                .iter()
                .fold((0.0f32, 0.0f32), |(sum, grow), size| match size {
                    GrowSize::Fixed(s) => (
                        sum + s.size(parent_size, &mut ChildSize::default(), ctx),
                        grow,
                    ),
                    GrowSize::Grow(s) => (
                        sum,
                        grow + s.size(parent_size, &mut ChildSize::default(), ctx),
                    ),
                });

        let col_gap = self
            .gap_columns
            .size(parent_size, &mut ChildSize::default(), ctx);
        let total_col_gap = col_gap * (self.template_columns.len().saturating_sub(1) as f32);

        let row_gap = self
            .gap_rows
            .size(parent_size, &mut ChildSize::default(), ctx);
        let total_row_gap = row_gap * (self.template_rows.len().saturating_sub(1) as f32);

        let col_px_per_grow = if col_grow_sum > 0.0 {
            ((parent_size[0] - col_px_sum - total_col_gap) / col_grow_sum).max(0.0)
        } else {
            0.0
        };
        let row_px_per_grow = if row_grow_sum > 0.0 {
            ((parent_size[1] - row_px_sum - total_row_gap) / row_grow_sum).max(0.0)
        } else {
            0.0
        };

        let mut column_ranges = Vec::with_capacity(self.template_columns.len());
        let mut x = 0.0f32;
        for size in &self.template_columns {
            let w = match size {
                GrowSize::Fixed(s) => s.size(parent_size, &mut ChildSize::default(), ctx),
                GrowSize::Grow(s) => {
                    col_px_per_grow * s.size(parent_size, &mut ChildSize::default(), ctx)
                }
            };
            column_ranges.push([x, x + w]);
            x += w + col_gap;
        }

        let mut row_ranges = Vec::with_capacity(self.template_rows.len());
        let mut y = 0.0f32;
        for size in &self.template_rows {
            let h = match size {
                GrowSize::Fixed(s) => s.size(parent_size, &mut ChildSize::default(), ctx),
                GrowSize::Grow(s) => {
                    row_px_per_grow * s.size(parent_size, &mut ChildSize::default(), ctx)
                }
            };
            row_ranges.push([y, y + h]);
            y += h + row_gap;
        }

        (column_ranges, row_ranges)
    }

    fn child_arrangement(
        setting: &GridChildSetting,
        column_ranges: &[[f32; 2]],
        row_ranges: &[[f32; 2]],
    ) -> ([f32; 2], Matrix4<f32>) {
        let col_start = column_ranges
            .get(setting.column[0])
            .map(|r| r[0])
            .unwrap_or(0.0);
        let col_end = column_ranges
            .get(setting.column[1].saturating_sub(1))
            .map(|r| r[1])
            .unwrap_or(col_start);
        let row_start = row_ranges.get(setting.row[0]).map(|r| r[0]).unwrap_or(0.0);
        let row_end = row_ranges
            .get(setting.row[1].saturating_sub(1))
            .map(|r| r[1])
            .unwrap_or(row_start);

        let child_size = [
            (col_end - col_start).max(0.0),
            (row_end - row_start).max(0.0),
        ];
        let affine = Matrix4::new_translation(&nalgebra::Vector3::new(col_start, row_start, 0.0));
        (child_size, affine)
    }
}

impl Widget for GridWidget {
    type View = Grid;

    fn update(&mut self, view: &Grid, ctx: &UiContext) -> WidgetInteractionResult {
        let settings_changed = self.template_columns != view.template_columns
            || self.template_rows != view.template_rows
            || self.gap_columns != view.gap_columns
            || self.gap_rows != view.gap_rows;

        self.template_columns = view.template_columns.clone();
        self.template_rows = view.template_rows.clone();
        self.gap_columns = view.gap_columns.clone();
        self.gap_rows = view.gap_rows.clone();

        let mut layout_needed = settings_changed || self.children.len() != view.items.len();

        let existing = self.children.len().min(view.items.len());

        for i in 0..existing {
            let new_setting = GridChildSetting {
                column: view.items[i].column,
                row: view.items[i].row,
            };
            if self.children[i].1 != new_setting {
                self.children[i].1 = new_setting;
                layout_needed = true;
            }
            match self.children[i]
                .0
                .try_update(view.items[i].item.as_ref(), ctx)
            {
                Ok(WidgetInteractionResult::LayoutNeeded) => layout_needed = true,
                Ok(_) => {}
                Err(_) => {
                    self.children[i].0 = view.items[i].item.build(ctx);
                    layout_needed = true;
                }
            }
        }

        self.children.truncate(view.items.len());

        for gi in view.items.iter().skip(existing) {
            self.children.push((
                gi.item.build(ctx),
                GridChildSetting {
                    column: gi.column,
                    row: gi.row,
                },
            ));
            layout_needed = true;
        }

        if layout_needed {
            WidgetInteractionResult::LayoutNeeded
        } else {
            WidgetInteractionResult::NoChange
        }
    }

    fn device_input(
        &mut self,
        bounds: [f32; 2],
        event: &DeviceEvent,
        ctx: &UiContext,
    ) -> WidgetInteractionResult {
        let (column_ranges, row_ranges) = self.calc_grid_layout(bounds, ctx);
        let mut result = WidgetInteractionResult::NoChange;
        for (pod, setting) in &mut self.children {
            let (child_size, affine) =
                GridWidget::child_arrangement(setting, &column_ranges, &row_ranges);
            let child_event = event.transform(affine);
            match pod.device_input(child_size, &child_event, ctx) {
                WidgetInteractionResult::LayoutNeeded => {
                    result = WidgetInteractionResult::LayoutNeeded
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
        if self.template_columns.is_empty() || self.template_rows.is_empty() {
            return [0.0, 0.0];
        }
        let parent_size = [constraints.max_width(), constraints.max_height()];
        let (column_ranges, row_ranges) = self.calc_grid_layout(parent_size, ctx);
        let total_width = column_ranges.last().map(|r| r[1]).unwrap_or(0.0);
        let total_height = row_ranges.last().map(|r| r[1]).unwrap_or(0.0);
        [
            total_width.clamp(constraints.min_width(), constraints.max_width()),
            total_height.clamp(constraints.min_height(), constraints.max_height()),
        ]
    }

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode {
        let (column_ranges, row_ranges) = self.calc_grid_layout(bounds, ctx);
        let mut render_node = RenderNode::new();
        for (pod, setting) in &mut self.children {
            let (child_size, affine) =
                GridWidget::child_arrangement(setting, &column_ranges, &row_ranges);
            let child_node = pod.render(child_size, ctx);
            render_node = render_node.add_child(child_node, affine);
        }
        render_node
    }
}
