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

use crate::color_rect::RectGeometry;

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
    Path { path: PathBuf, target: [u32; 2] },
    Bytes { ptr: usize, len: usize, target: [u32; 2] },
}

impl ImageCacheKey {
    fn new(source: &ImageSource, target: [u32; 2]) -> Self {
        match source {
            ImageSource::Path(path) => ImageCacheKey::Path {
                path: path.clone(),
                target,
            },
            ImageSource::Bytes(bytes) => ImageCacheKey::Bytes {
                ptr: bytes.as_ptr() as usize,
                len: bytes.len(),
                target,
            },
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

/// Build a `RenderItem` fitting `source` within `box_w`×`box_h` (CSS
/// `object-fit: contain`), decoding/resizing/uploading at most once per
/// distinct `(source, box size)` pair via `image_ctx`.
fn image_render_item(image_ctx: ImageCtx, source: ImageSource, box_w: f32, box_h: f32) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| {
        let mut node = RenderNode::new();
        if box_w <= 0.0 || box_h <= 0.0 {
            return node;
        }
        let target = [box_w.ceil() as u32, box_h.ceil() as u32];
        let key = ImageCacheKey::new(&source, target);

        if let Some(cached) = image_ctx.0.lock().get(&key) {
            return compose(node, cached, box_w, box_h);
        }

        let Some(decoded) = decode(&source) else {
            return node;
        };
        let fitted = decoded.resize(target[0], target[1], image::imageops::FilterType::Triangle);
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

/// Centre `(region, fitted_size)` within a `box_w`×`box_h` box (the
/// letterbox/pillarbox effect of `object-fit: contain` when the fitted
/// image's aspect ratio doesn't match the box's).
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

/// A fixed-size image, decoded from a file path or in-memory bytes.
pub struct Image {
    key: Key,
    source: ImageSource,
    w: f32,
    h: f32,
}

impl Image {
    /// Decode from a filesystem path, displayed within a `w`×`h` box.
    pub fn from_path(path: impl Into<PathBuf>, w: f32, h: f32) -> Self {
        Self {
            key: Key::Auto,
            source: ImageSource::Path(path.into()),
            w,
            h,
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
            source: ImageSource::Bytes(bytes),
            w,
            h,
        }
    }

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
        image_render_item(image_ctx, self.source.clone(), self.w, self.h)
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
            self.geometry(),
            LayoutDispatch::of::<RectGeometry>(),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        let item = self.rebuild_render_item(entity);
        entity.insert(item);
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        let mut changed = false;
        if let Some(mut c) = entity.get_mut::<ImageContent>() {
            changed |= c.set_if_neq(ImageContent(self.source.clone()));
        }
        if let Some(mut g) = entity.get_mut::<RectGeometry>() {
            changed |= g.set_if_neq(self.geometry());
        }
        if changed {
            let item = self.rebuild_render_item(entity);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}
