//! UI-dependent values are resolved before immutable GPU painters are captured.
//! Painters record into a widget's logical image; they do not submit or own atlas slots.
pub mod image;
pub mod polygon;
pub mod solid_box;
pub mod viewport_clear;
// Existing disabled text modules remain outside this migration.
use matcha_tree::ui_tree::{
    context::UiContext,
    metrics::{Constraints, QRect},
};
use render_interface::{PrepareResult, TexturePrepareContext};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
#[derive(Clone)]
pub struct PreparedStyle {
    ids: Arc<[u64]>,
    paint: Arc<dyn for<'a> Fn(TexturePrepareContext<'a>) -> PrepareResult + Send + Sync>,
}
impl PreparedStyle {
    pub fn new(
        paint: impl for<'a> Fn(TexturePrepareContext<'a>) -> PrepareResult + Send + Sync + 'static,
    ) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            ids: Arc::from([NEXT
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .expect("style identity exhausted")]),
            paint: Arc::new(paint),
        }
    }
    pub(crate) fn ids(&self) -> &[u64] {
        &self.ids
    }
    pub(crate) fn record(&self, context: TexturePrepareContext<'_>) -> PrepareResult {
        (self.paint)(context)
    }
}
/// Resolve borrowed UI state into an owned painter. Keep its identity stable
/// while its logical drawing remains unchanged. It must initialize only the
/// pixels it covers; Buffer initializes the complete output before all painters.
pub trait Style: utils::MaybeSendSync {
    fn required_region(&self, constraints: &Constraints, ctx: &UiContext) -> Option<QRect>;
    fn is_inside(&self, position: [f32; 2], bounds: [f32; 2], ctx: &UiContext) -> bool {
        self.required_region(&Constraints::from_boundary(bounds), ctx)
            .is_some_and(|r| r.contains(position))
    }
    fn prepare(
        &self,
        boundary: [f32; 2],
        offset: [f32; 2],
        ctx: &UiContext,
    ) -> Option<PreparedStyle>;
}
impl Style for Vec<Arc<dyn Style>> {
    fn required_region(&self, constraints: &Constraints, ctx: &UiContext) -> Option<QRect> {
        self.iter()
            .filter_map(|s| s.required_region(constraints, ctx))
            .reduce(|a, b| a.union(&b))
    }
    fn is_inside(&self, p: [f32; 2], bounds: [f32; 2], ctx: &UiContext) -> bool {
        self.iter().any(|s| s.is_inside(p, bounds, ctx))
    }
    fn prepare(
        &self,
        bounds: [f32; 2],
        offset: [f32; 2],
        ctx: &UiContext,
    ) -> Option<PreparedStyle> {
        let painters: Vec<_> = self
            .iter()
            .filter_map(|s| s.prepare(bounds, offset, ctx))
            .collect();
        if painters.is_empty() {
            return None;
        }
        // Composition is ordered painting, so retain the flattened leaf identities.
        // Creating a fresh composite wrapper must not invalidate immutable pixels.
        let ids = painters
            .iter()
            .flat_map(|p| p.ids().iter().copied())
            .collect::<Vec<_>>()
            .into();
        Some(PreparedStyle {
            ids,
            paint: Arc::new(move |context| record_all(&painters, context)),
        })
    }
}
pub(crate) fn record_all(
    painters: &[PreparedStyle],
    context: TexturePrepareContext<'_>,
) -> PrepareResult {
    for paint in painters {
        paint.record(TexturePrepareContext {
            gpu: render_interface::GpuPrepareContext {
                device: context.gpu.device,
                encoder: &mut *context.gpu.encoder,
                snapshot: context.gpu.snapshot,
            },
            target: render_interface::TextureTarget {
                desc: context.target.desc,
                texture: context.target.texture,
                view: context.target.view,
            },
        })?;
    }
    Ok(())
}
