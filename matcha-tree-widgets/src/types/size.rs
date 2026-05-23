use std::sync::Arc;

use matcha_tree::ui_tree::context::UiContext;

pub struct ChildSize<'a> {
    get_size: Box<dyn FnMut() -> [f32; 2] + 'a>,
    cached_child_size: std::cell::Cell<Option<[f32; 2]>>,
}

impl Default for ChildSize<'_> {
    fn default() -> Self {
        Self {
            get_size: Box::new(|| [0.0, 0.0]),
            cached_child_size: std::cell::Cell::new(Some([0.0, 0.0])),
        }
    }
}

impl<'a> ChildSize<'a> {
    pub fn new<F>(get_size: F) -> Self
    where
        F: FnMut() -> [f32; 2] + 'a,
    {
        Self {
            get_size: Box::new(get_size),
            cached_child_size: std::cell::Cell::new(None),
        }
    }

    pub fn with_size(size: [f32; 2]) -> Self {
        Self {
            get_size: Box::new(move || size),
            cached_child_size: std::cell::Cell::new(Some(size)),
        }
    }
}

impl ChildSize<'_> {
    pub fn get(&mut self) -> [f32; 2] {
        if let Some(size) = self.cached_child_size.get() {
            size
        } else {
            let size = (self.get_size)();
            self.cached_child_size.set(Some(size));
            size
        }
    }
}

/// Closure trait used to compute a size from parent size, child size and
/// context. `MaybeSendSync` supplies the platform-conditional `Send + Sync`
/// bound.
pub trait SizeFn:
    for<'a, 'b> Fn([f32; 2], &'a mut ChildSize<'b>, &UiContext) -> f32 + utils::MaybeSendSync
{
}
impl<F> SizeFn for F where
    F: for<'a, 'b> Fn([f32; 2], &'a mut ChildSize<'b>, &UiContext) -> f32 + utils::MaybeSendSync
{
}

/// Calculate size from parent size child size and context.
#[derive(Clone)]
pub struct Size {
    f: Arc<dyn SizeFn>,
}

impl Size {
    /// Specify size in pixels.
    pub fn px(px: f32) -> Self {
        Self {
            f: Arc::new(move |_, _, _| px),
        }
    }

    /// Specify size in inches.
    pub fn inch(inch: f32) -> Self {
        Self {
            f: Arc::new(move |_, _, ctx: &UiContext| inch * ctx.dpi().unwrap_or(1.0) as f32),
        }
    }

    /// Specify size in points.
    pub fn point(point: f32) -> Self {
        Self {
            f: Arc::new(move |_, _, ctx: &UiContext| {
                point * ctx.dpi().unwrap_or(1.0) as f32 / 72.0
            }),
        }
    }

    /// Specify size in magnification of parent width.
    pub fn parent_w(mag: f32) -> Self {
        Self {
            f: Arc::new(move |parent_size, _, _| parent_size[0] * mag),
        }
    }

    pub fn parent_h(mag: f32) -> Self {
        Self {
            f: Arc::new(move |parent_size, _, _| parent_size[1] * mag),
        }
    }

    pub fn child_w(mag: f32) -> Self {
        Self {
            f: Arc::new(move |_, child_size, _| child_size.get()[0] * mag),
        }
    }

    pub fn child_h(mag: f32) -> Self {
        Self {
            f: Arc::new(move |_, child_size, _| child_size.get()[1] * mag),
        }
    }

    /// Specify size in magnification of font size.
    pub fn em(em: f32) -> Self {
        // WidgetContext does not currently expose font size; use a reasonable default (16.0px).
        Self {
            f: Arc::new(move |_, _, _ctx| em * 16.0),
        }
    }

    // /// Specify size in magnification of root font size.
    // pub fn rem(rem: f32) -> Self {
    //     Self {
    //         f: Arc::new(move |_, _, ctx| rem * ctx.root_font_size()),
    //     }
    // }

    /// Specify size in magnification of viewport width.
    pub fn vw(vw: f32) -> Self {
        Self {
            f: Arc::new(move |_, _, ctx: &UiContext| {
                vw * ctx.viewport_size().map(|v| v[0]).unwrap_or(0.0)
            }),
        }
    }

    /// Specify size in magnification of viewport height.
    pub fn vh(vh: f32) -> Self {
        Self {
            f: Arc::new(move |_, _, ctx: &UiContext| {
                vh * ctx.viewport_size().map(|v| v[1]).unwrap_or(0.0)
            }),
        }
    }

    /// Specify size in magnification of vmax.
    pub fn vmax(vmax: f32) -> Self {
        Self {
            f: Arc::new(move |_, _, ctx: &UiContext| {
                let vs = ctx.viewport_size().unwrap_or([0.0, 0.0]);
                vmax * vs[0].max(vs[1])
            }),
        }
    }

    /// Specify size in magnification of vmin.
    pub fn vmin(vmin: f32) -> Self {
        Self {
            f: Arc::new(move |_, _, ctx: &UiContext| {
                let vs = ctx.viewport_size().unwrap_or([0.0, 0.0]);
                vmin * vs[0].min(vs[1])
            }),
        }
    }
}

impl Size {
    /// Specify size with a custom function.
    pub fn from_size<F>(f: F) -> Self
    where
        F: Fn([f32; 2], &mut ChildSize, &UiContext) -> f32 + utils::MaybeSendSync + 'static,
    {
        Self { f: Arc::new(f) }
    }
}

impl Size {
    pub fn size(&self, parent_size: [f32; 2], child_size: &mut ChildSize, ctx: &UiContext) -> f32 {
        (self.f)(parent_size, child_size, ctx)
    }

    // pub fn constraints(
    //     &self,
    //     constraints: &Constraints,
    //     child_size: &mut ChildSize,
    //     ctx: &WidgetContext,
    // ) -> [f32; 2] {
    //     todo!()
    // }
}

impl PartialEq for Size {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.f, &other.f)
    }
}
