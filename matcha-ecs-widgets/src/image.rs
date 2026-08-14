//! `Image` — a fixed-size leaf widget displaying a decoded raster image,
//! fit within its box preserving aspect ratio (CSS `object-fit: contain`).
//!
//! `(w, h)` are mandatory constructor arguments, not an optional `.size()`
//! with a natural-size default — deliberately, so `measure()` never needs
//! decoded image dimensions and decode never happens on the layout pass
//! (which runs every frame, not just on view changes). Layout reuses
//! `RectGeometry`/`LayoutDispatch::of::<RectGeometry>()` verbatim, same as
//! `ColorRect`/`Checkbox`.
//!
//! Decode+resize+upload happens synchronously inside the `RenderItem`
//! builder (matching `Text`'s existing "shape on first render-item build, no
//! async" precedent) but is cached by `(source identity, display size)` in a
//! lazily-inserted `ImageCtx` resource — mirroring `Text`'s `FontCtx`
//! stencil cache. This matters because `matcha-ecs/src/systems.rs`'s
//! `invalidate_on_layout_change` invalidates *every* `RenderItem` on any
//! `LayoutOutput` change, including a pure reposition with unchanged size;
//! without this cache, any reflow near an `Image` would force a full
//! re-decode. Resizing before upload is also a correctness requirement, not
//! just an optimisation: atlas pages are fixed 4096×4096, and `allocate()`
//! fails outright above that, so a natural-resolution large photo would
//! otherwise hard-fail.
//!
//! Object-fit: v1 supports exactly `contain` — this is
//! `image::DynamicImage::resize`'s documented behaviour verbatim, so no
//! custom fit math is needed. `fill`/`cover` are not implemented.
//!
//! **`ImageSource::Bytes`'s cache/change-detection identity is the `Arc<[u8]>`'s
//! pointer, not its byte content** (`ImageSource`'s `PartialEq` uses
//! `Arc::ptr_eq`, and `ImageCacheKey::Bytes` keys on `.as_ptr()`). This is
//! deliberate — a deep byte compare on every `patch()`/cache-lookup would be
//! its own perf problem for a multi-megabyte image — but it means the caller
//! must hold one `Arc<[u8]>` and pass `.clone()`s of it across `view()` calls
//! (`Image::from_bytes` takes an owned `Arc<[u8]>`, not `impl Into<Arc<[u8]>>`,
//! specifically to make this unavoidable — see its doc comment). Passing a
//! `&'static [u8]` from `include_bytes!` directly at each `view()` call site
//! would implicitly allocate a fresh `Arc` every time, defeating both the
//! decode cache *and* `patch`'s change-detection on every single re-render,
//! not just real content changes.
//!
//! Async decode (e.g. via `matcha_ecs::task::spawn_task`) is explicitly
//! deferred: reporting completion back into the ECS world needs the *app's*
//! own `ModelHandle::update`/`Msg` routing, a bigger cross-cutting design
//! than this widget alone.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, resource::Resource,
    world::EntityWorldMut,
};
use gpu_utils::texture_atlas::AtlasRegion;
use nalgebra::{Matrix4, Vector3};
use parking_lot::Mutex;
use renderer::RenderNode;

use matcha_ecs::{
    components::{
        render::{RenderCtx, RenderItem},
        view::Key,
    },
    layout::LayoutDispatch,
    view::Widget,
};

use crate::sizing::Sizing;
use crate::sizing::RectGeometry;

/// Where an `Image`'s bytes come from. Identity (not content) is what
/// matters for change-detection and cache keying — see `PartialEq`/
/// `ImageCacheKey` below, both deliberately pointer/path-identity based
/// rather than a deep byte compare.
#[derive(Clone)]
pub enum ImageSource {
    Path(PathBuf),
    Bytes(Arc<[u8]>),
}

impl PartialEq for ImageSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ImageSource::Path(a), ImageSource::Path(b)) => a == b,
            (ImageSource::Bytes(a), ImageSource::Bytes(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

#[derive(Component, Clone, PartialEq)]
struct ImageContent(ImageSource);

#[derive(Clone, PartialEq, Eq, Hash)]
enum ImageCacheKey {
    Path {
        path: PathBuf,
        target: [u32; 2],
        fit: ObjectFit,
    },
    Bytes {
        ptr: usize,
        len: usize,
        target: [u32; 2],
        fit: ObjectFit,
    },
}

impl ImageCacheKey {
    fn new(source: &ImageSource, target: [u32; 2], fit: ObjectFit) -> Self {
        match source {
            ImageSource::Path(path) => ImageCacheKey::Path {
                path: path.clone(),
                target,
                fit,
            },
            ImageSource::Bytes(bytes) => ImageCacheKey::Bytes {
                ptr: bytes.as_ptr() as usize,
                len: bytes.len(),
                target,
                fit,
            },
        }
    }
}

/// How the image fills the box it was given (CSS `object-fit`).
///
/// Each variant is one call into the `image` crate's resize family, so the
/// semantics are exactly that crate's documented behaviour rather than fit
/// arithmetic maintained here.
///
/// CSS's `none` (natural size, overflowing) is deliberately absent: the atlas
/// pages are a fixed 4096x4096 and `allocate` hard-fails above that, so a
/// natural-resolution photo would not merely overflow, it would fail to draw
/// at all. [`ScaleDown`](Self::ScaleDown) is the usable half of that intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ObjectFit {
    /// Fit inside the box, preserving aspect ratio; the leftover is letterboxed.
    #[default]
    Contain,
    /// Stretch to the box exactly, ignoring aspect ratio.
    Fill,
    /// Cover the box, preserving aspect ratio; the overflow is cropped.
    Cover,
    /// Natural size, shrunk to fit only if it is larger than the box.
    ScaleDown,
}

impl ObjectFit {
    /// Produce the pixels to upload for a `target`-sized box.
    fn apply(self, decoded: &image::DynamicImage, target: [u32; 2]) -> image::DynamicImage {
        const FILTER: image::imageops::FilterType = image::imageops::FilterType::Triangle;
        match self {
            ObjectFit::Contain => decoded.resize(target[0], target[1], FILTER),
            ObjectFit::Fill => decoded.resize_exact(target[0], target[1], FILTER),
            // Crops to the box, so the result is exactly `target` and the
            // centring below is a no-op — as it should be for `cover`.
            ObjectFit::Cover => decoded.resize_to_fill(target[0], target[1], FILTER),
            ObjectFit::ScaleDown => {
                let (w, h) = (decoded.width(), decoded.height());
                if w <= target[0] && h <= target[1] {
                    decoded.clone()
                } else {
                    decoded.resize(target[0], target[1], FILTER)
                }
            }
        }
    }
}

/// World resource caching decoded-and-fitted images by `(source, display
/// size)`, keyed via [`ImageCacheKey`]. Lazily inserted on first use, exactly
/// like `text.rs`'s `FontCtx`. Unbounded, same accepted tradeoff as
/// `FontCtx`'s glyph stencil cache — fine for v1, revisit only if a real app
/// displays many distinct large images over a long session.
#[derive(Resource, Clone)]
struct ImageCtx(Arc<Mutex<HashMap<ImageCacheKey, (AtlasRegion, [f32; 2]), fxhash::FxBuildHasher>>>);

impl ImageCtx {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::default())))
    }
}

fn decode(source: &ImageSource) -> Option<image::DynamicImage> {
    match source {
        ImageSource::Path(path) => match image::open(path) {
            Ok(img) => Some(img),
            Err(e) => {
                log::error!("Image decode failed for {path:?}: {e}");
                None
            }
        },
        ImageSource::Bytes(bytes) => match image::load_from_memory(bytes) {
            Ok(img) => Some(img),
            Err(e) => {
                log::error!("Image decode failed: {e}");
                None
            }
        },
    }
}

/// Build a `RenderItem` fitting `source` within the layout-allocated box
/// (`ctx.size` — which a parent layout may have stretched beyond the declared
/// `w`×`h`; CSS `object-fit: contain`), decoding/resizing/uploading at most
/// once per distinct `(source, box size)` pair via `image_ctx`.
fn image_render_item(image_ctx: ImageCtx, source: ImageSource, fit: ObjectFit) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| {
        let [box_w, box_h] = ctx.size;
        let mut node = RenderNode::new();
        if box_w <= 0.0 || box_h <= 0.0 {
            return node;
        }
        let target = [box_w.ceil() as u32, box_h.ceil() as u32];
        let key = ImageCacheKey::new(&source, target, fit);

        if let Some(cached) = image_ctx.0.lock().get(&key) {
            return compose(node, cached, box_w, box_h);
        }

        let Some(decoded) = decode(&source) else {
            return node;
        };
        let fitted = fit.apply(&decoded, target);
        let rgba = fitted.to_rgba8();
        let (w, h) = rgba.dimensions();
        if w == 0 || h == 0 {
            return node;
        }

        let region = match ctx.texture_atlas.allocate(ctx.device, ctx.queue, [w, h]) {
            Ok(region) => region,
            Err(e) => {
                log::error!("Image atlas allocation failed: {e}");
                return node;
            }
        };
        // `.to_rgba8()`'s bytes are already sRGB-gamma-encoded by convention
        // (matching the atlas's Rgba8UnormSrgb format), unlike `ColorRect`/
        // `Text`'s linear-float colours which need `linear_to_srgb_u8`
        // before a raw `write_data` — no conversion needed here.
        if let Err(e) = region.write_data(ctx.queue, rgba.as_raw()) {
            log::error!("Image upload failed: {e}");
            return node;
        }

        let entry = (region, [w as f32, h as f32]);
        image_ctx.0.lock().insert(key, entry.clone());
        node = compose(node, &entry, box_w, box_h);
        node
    })
}

/// Centre `(region, fitted_size)` within its box.
///
/// This is what produces `contain`'s letterbox/pillarbox bars. For `fill` and
/// `cover` the fitted size already equals the box, so the offset is zero and
/// this costs nothing; for `scale-down` of a small image it centres it.
fn compose(mut node: RenderNode, (region, fitted_size): &(AtlasRegion, [f32; 2]), box_w: f32, box_h: f32) -> RenderNode {
    let offset = Matrix4::new_translation(&Vector3::new(
        ((box_w - fitted_size[0]) / 2.0).max(0.0),
        ((box_h - fitted_size[1]) / 2.0).max(0.0),
        0.0,
    ));
    let image_node = RenderNode::new().with_texture(region.clone(), *fitted_size, Matrix4::identity());
    node.push_child(image_node, offset);
    node
}

/// The declared [`ObjectFit`], carried so `patch` can detect a change to it.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ImageFit(pub ObjectFit);

/// A fixed-size image, decoded from a file path or in-memory bytes.
pub struct Image {
    key: Key,
    sizing: Sizing,
    source: ImageSource,
    w: f32,
    h: f32,
    fit: ObjectFit,
}

impl Image {
    /// Decode from a filesystem path, displayed within a `w`×`h` box.
    ///
    /// **Native only in practice.** `std::fs` compiles on wasm but every call
    /// returns `Unsupported`, so on the web this degrades to a log line and
    /// nothing drawn. Use [`Image::from_bytes`] with `include_bytes!` there.
    pub fn from_path(path: impl Into<PathBuf>, w: f32, h: f32) -> Self {
        Self {
            key: Key::Auto,
            sizing: Sizing::default(),
            source: ImageSource::Path(path.into()),
            w,
            h,
            fit: ObjectFit::default(),
        }
    }

    /// Decode from in-memory encoded image bytes (PNG/JPEG/...), displayed
    /// within a `w`×`h` box.
    ///
    /// Takes an owned `Arc<[u8]>` rather than `impl Into<Arc<[u8]>>`
    /// deliberately: an `impl Into` here would silently accept a plain
    /// `&[u8]`/`Vec<u8>` and allocate a **fresh** `Arc` (copying every byte)
    /// on every call — since `ImageCacheKey`/`ImageContent`'s identity is
    /// this `Arc`'s pointer (see the module docs), a widget declared fresh
    /// from a `&'static [u8]` every `view()` call (the natural pattern for
    /// `include_bytes!`-embedded assets) would defeat the decode cache on
    /// every single re-render, not just image-content changes. Construct the
    /// `Arc<[u8]>` once (e.g. a `std::sync::LazyLock`) and pass a `.clone()`
    /// (a cheap refcount bump that preserves pointer identity) on every
    /// `view()` call instead.
    pub fn from_bytes(bytes: Arc<[u8]>, w: f32, h: f32) -> Self {
        Self {
            key: Key::Auto,
            sizing: Sizing::default(),
            source: ImageSource::Bytes(bytes),
            w,
            h,
            fit: ObjectFit::default(),
        }
    }

    /// How the image fills its box (CSS `object-fit`). Default
    /// [`Contain`](ObjectFit::Contain).
    pub fn fit(mut self, fit: ObjectFit) -> Self {
        self.fit = fit;
        self
    }

    crate::sizing_builders!();

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    fn geometry(&self) -> RectGeometry {
        RectGeometry {
            w: self.w,
            h: self.h,
        }
    }

    fn rebuild_render_item(&self, entity: &mut EntityWorldMut) -> RenderItem {
        let image_ctx = entity.world_scope(|world| world.get_resource_or_insert_with(ImageCtx::new).clone());
        image_render_item(image_ctx, self.source.clone(), self.fit)
    }
}

impl Widget for Image {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        // Unlike `ColorRect`, the `RenderItem` can't be built here: `ImageCtx`
        // is a world resource and `bundle()` has no world access. Built in
        // `after_spawn` instead, mirroring `Text`/`Button`.
        (
            ImageContent(self.source.clone()),
            ImageFit(self.fit),
            self.geometry(),
            self.sizing,
            LayoutDispatch::of::<RectGeometry>(),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        let item = self.rebuild_render_item(entity);
        entity.insert(item);
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        self.sync_sizing(entity);
        let mut changed = false;
        if let Some(mut c) = entity.get_mut::<ImageContent>() {
            changed |= c.set_if_neq(ImageContent(self.source.clone()));
        }
        if let Some(mut g) = entity.get_mut::<RectGeometry>() {
            changed |= g.set_if_neq(self.geometry());
        }
        if let Some(mut f) = entity.get_mut::<ImageFit>() {
            changed |= f.set_if_neq(ImageFit(self.fit));
        }
        if changed {
            let item = self.rebuild_render_item(entity);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}
