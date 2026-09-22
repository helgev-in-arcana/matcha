# Text and fonts

Two independent text stacks live in `matcha-ecs-widgets`, on purpose. The core depends on neither.

| Widget | Engine | Shaping | Fallback | Use it when |
|---|---|---|---|---|
| `Text` | suzuri (path dep) + fontdue | kerning only | **none** | Fixed, known, Latin content. Reference/fallback implementation with in-house layout |
| `RichText` | parley (HarfRust) + swash | full | fontique | **Anything showing arbitrary runtime text.** Per-span styling, decorations |
| `TextBox` | parley `PlainEditor` | full | fontique | Editing, IME, selection, caret |

**Any non-Latin character in `Text` renders as tofu.** The showcase displays echoed runtime values
with `RichText` for exactly this reason. `Button`'s label goes through `Text`'s suzuri path.

Both stacks are kept because parley has known layout-reproducibility issues and may be replaced;
`Text` is the in-house-layout fallback. parley is fair game only inside `rich_text.rs` / `text_box.rs`
(and the shared `draw_parley_layout`).

## The compositing trick, shared by everything

A glyph is a **1×1 solid-colour tint quad** (CPU colour bitmap) masked by its **coverage bitmap**
(CPU coverage bitmap, uploaded as R8Unorm), composited by `RenderNode::with_stencil`. fontdue's
`rasterize_indexed` and swash's `Render::format(Format::Alpha)` produce the same thing, so the GPU
path is identical for both stacks — and it is the same path rounded rectangles take
(`shape.rs`). See [rendering.md](rendering.md).

Decorations (underline, strikethrough) are solid rectangles: `.with_texture(..)` with **no** mask,
and deliberately not a real render pass — a 1–2px-tall region is exactly the tiny-viewport shape that
rasterises incorrectly through one.

## Layout ↔ render, and what is not cached

`measure`, `arrange` and the `RenderItem` builder each **independently re-shape from scratch**. The
only value passed between stages is the resolved wrap width, through a shared atomic cell
(`live.rs`). That is enough to make the generic `invalidate_on_layout_change` cover text reflow with
zero new systems, since the closure keeps re-reading the live width instead of needing a rebuild.

Passing the shaped glyph list between stages is an obvious future optimisation, not done.

Per-glyph rasterisation *is* cached: `RichText` keys on font blob id + font index + glyph id +
quantized size + a hash of variation coords, in a bounded `glyph-cache` LRU (capacity 1024, with
per-batch eviction protection; a "batch" is one `RenderItem` build, not one frame). `Text`'s cache is
an unbounded `HashMap`, fine for its fixed-content use.

**Known `GlyphKey` gap**: `fontique::Synthesis` (synthetic bold/oblique when the fallback chain has
no true face) is never applied — supporting it needs a new key field. Requesting such a weight/style
silently has no visual effect. Accepted tradeoff.

## `TextBox`

`text_box.rs`'s docs are thorough; the points most likely to be re-derived wrongly:

- **`PlainEditor` must not be the `Layout` impl type.** `LayoutDispatch::of::<L>()` clones `L` out of
  the world on *every* measure and arrange — that would deep-copy the buffer and layout twice per
  frame. `TextBoxLayout { w, h }` is the layout; the editor is a separate `Arc<Mutex<..>>` component.
- **Fixed size.** `w` feeds `set_width`, which kills the "wrap width needs measure, measure needs
  wrap width" circularity outright.
- **Hybrid value sync**: the buffer is overwritten only when the *declared* value changed, so
  re-declaring an unchanged value never clobbers what the user is typing. The one widget that is not
  purely declarative, by necessity.
- **IME `Commit` must `clear_compose()` before inserting** — the preedit is still in the buffer, and
  `clear_compose` also restores the caret to where composition began. `finish_compose` accepts the
  preedit verbatim, which is wrong when the platform hands us the final text separately. A headless
  Japanese-composition test caught this.
- **Keys are swallowed while `is_composing()`**, or every composed character is typed twice.
- **The confirm binding is a predicate**, `ConfirmKey(fn(&KeyInput) -> bool)`, not a key enum: the
  useful chord differs by context. `confirm_on_ctrl_enter` (default) and `confirm_on_enter` ship.
- v1 is **multi-line only** — a single-line field needs horizontal scrolling.
- `OnTextUpdate`/`OnTextConfirm`/`ConfirmKey` do **not** derive `PartialEq`: comparing fn pointers is
  not meaningful, so `patch` assigns them outright.

## Fonts on this branch

Text loads native system fonts through suzuri, and RichText/TextBox through parley/fontique.
This main-based checkout has no matcha-web crate, font.rs or WithDefaultFont API. The previous
web-font notes described another branch and must not be used as current API instructions here.

CPU glyph Bitmap caches survive GPU cache eviction. SceneBuilder registers upload sources lazily;
renderer residency and UI glyph-cache eviction are independent. Colour/coverage sharing is retained.
