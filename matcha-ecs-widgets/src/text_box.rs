//! An editable, IME-capable multi-line text box.
//!
//! # Where the text lives
//!
//! Every other widget here is fully declarative: the app passes the current
//! value on each `view()` call and the widget holds nothing. A text box cannot
//! work that way — it owns a caret, a selection, and an in-progress IME
//! composition, none of which the app should have to model. So the editing
//! state lives on the entity, in a [`parley::PlainEditor`].
//!
//! The app is not cut out of the loop. It declares a `value`, and whenever
//! *that declared value changes* the editor is overwritten with it (see
//! [`DeclaredValue`]); re-declaring an unchanged value never clobbers what the
//! user is typing. In the other direction the widget emits messages built from
//! the current text, and which of those an app listens to is its choice:
//! [`on_update`](TextBox::on_update) for every keystroke, or
//! [`on_confirm`](TextBox::on_confirm) for an explicit confirm and focus loss.
//!
//! # parley's blast radius
//!
//! parley is an implementation detail *of this file*. The keyboard and IME
//! events arriving here (`KeyInput`, `ImeEvent`) are matcha-window types
//! carrying owned strings and byte offsets, and the caret rectangle handed back
//! to the core (`ImeCursorArea`) is a plain `[f32; 4]`. Nothing on the delivery
//! path names a text engine, so parley can be replaced here without touching
//! `matcha-ecs` or `matcha-window`.
//!
//! # v1 limits
//!
//! - **Multi-line only.** The box wraps at its own width. A single-line field
//!   needs horizontal scrolling, which needs real clipping, which this renderer
//!   does not have yet. Enter therefore inserts a newline by default and
//!   confirmation is bound to Ctrl+Enter — see
//!   [`confirm_key`](TextBox::confirm_key) to change that.
//! - **Overflow is clipped** by the `Clip` marker on the widget's own entity: the
//!   content box is dropped rather than cut, so one straddling the edge pops
//!   instead of sliding.
//! - No clipboard (a separate delivery path — what is pasted is not necessarily
//!   text), no undo/redo (parley has none either), no per-span styling.

use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};

use bevy_ecs::{
    bundle::Bundle,
    change_detection::DetectChangesMut,
    component::Component,
    entity::Entity,
    schedule::{IntoScheduleConfigs, ScheduleConfigs},
    system::{Query, Res, ResMut, ScheduleSystem},
    world::EntityWorldMut,
};
use matcha_ecs::{
    components::{
        focus::{FocusDispatch, FocusPolicy, Focused},
        input::{
            ImeCursorArea, ImeDispatch, KeyDispatch, Message, Pickable, PointerDispatch,
            PointerInput, PointerPhase,
        },
        layout::{Clip, GlobalTransform},
        render::{RenderCtx, RenderItem},
        view::Key,
    },
    input::emit_message,
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, Measured},
    resources::{FrameTime, RedrawRequest},
    view::Widget,
};
use matcha_window::event::device_event::{ImeEvent, Key as LogicalKey, KeyInput, NamedKey};
use nalgebra::{Matrix4, Point3, Vector3};
use parking_lot::Mutex;
use parley::{PlainEditor, StyleProperty};
use renderer::RenderNode;

use crate::{
    color_rect::solid_rect_node,
    rich_text::{draw_parley_layout, paint_tint_region, ParleyFontCtx, RichTextBrush},
};

/// How long the caret stays visible, then invisible, per blink.
const CARET_BLINK_INTERVAL: f32 = 0.53;

/// Caret width in pixels.
const CARET_WIDTH: f32 = 1.5;

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// The editing state: text buffer, caret, selection and IME composition.
///
/// Behind an `Arc<Mutex<..>>` for two reasons, both imposed by the surrounding
/// framework:
///
/// 1. A `RenderItem`'s builder is a closure captured back at `bundle()`/`patch()`
///    time and runs on the render thread with no world access. Handing it a
///    clone of this `Arc` is how it reaches the laid-out text.
/// 2. `PlainEditor::refresh_layout` needs `&mut self`, but `LayoutCtx` only
///    hands out `&World`.
///
/// The main thread must not hold this lock while the render thread might build
/// a node — the same invariant `RenderItem::cache` already documents.
#[derive(Component, Clone)]
pub struct TextEditor(Arc<Mutex<PlainEditor<RichTextBrush>>>);

impl TextEditor {
    /// The committed text, excluding any in-progress IME preedit.
    pub fn text(&self) -> String {
        self.0.lock().text().chars().collect()
    }
}

/// The last `value` the app declared, so `patch` can tell an app-driven change
/// (overwrite the buffer) from the app re-declaring what it declared before
/// (leave the user's edits alone).
#[derive(Component, Clone, PartialEq, Debug)]
pub struct DeclaredValue(pub String);

/// Fixed box size. Deliberately small and cheap to clone: `LayoutDispatch`
/// clones the `Layout` component out of the world on **every** measure and
/// arrange, so putting `PlainEditor` itself here would deep-copy the whole
/// buffer and layout twice per frame.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct TextBoxLayout {
    pub w: f32,
    pub h: f32,
}

/// Draw-relevant styling, separate from the editor so `patch` can compare it.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct TextBoxStyle {
    pub font_size: f32,
    pub color: [f32; 4],
    pub background_color: [f32; 4],
    pub border_color: [f32; 4],
    pub focused_border_color: [f32; 4],
    pub selection_color: [f32; 4],
    pub caret_color: [f32; 4],
    pub border_width: f32,
    pub padding: f32,
}

/// Values the render builder needs but that change *after* it was captured.
///
/// The builder is a closure with no world access, so these travel through
/// shared cells it holds a clone of — the same side-channel `Text`/`RichText`
/// use for their wrap width. Written by [`default_systems`], read on every
/// rebuild.
#[derive(Component, Clone, Default)]
pub struct TextBoxLive {
    /// Vertical scroll offset in pixels, `f32` bit-cast.
    scroll: Arc<AtomicU32>,
    /// Whether the caret is in its visible blink phase.
    caret_visible: Arc<AtomicBool>,
    /// The size layout actually allocated, published by `arrange`. `f32`
    /// bit-cast. Zero until the first arrange.
    allocated: Arc<[AtomicU32; 2]>,
    /// The wrap width the editor was last shaped at. `PlainEditor::set_width`
    /// dirties the layout unconditionally and exposes no getter, so the applied
    /// value is tracked here to avoid re-shaping every frame.
    applied_wrap_width: Arc<AtomicU32>,
}

impl TextBoxLive {
    fn scroll(&self) -> f32 {
        f32::from_bits(self.scroll.load(Ordering::Relaxed))
    }

    fn set_scroll(&self, value: f32) -> bool {
        let bits = value.to_bits();
        self.scroll.swap(bits, Ordering::Relaxed) != bits
    }

    fn caret_visible(&self) -> bool {
        self.caret_visible.load(Ordering::Relaxed)
    }

    /// Returns whether the value actually changed.
    fn set_caret_visible(&self, value: bool) -> bool {
        self.caret_visible.swap(value, Ordering::Relaxed) != value
    }

    fn allocated(&self) -> [f32; 2] {
        [
            f32::from_bits(self.allocated[0].load(Ordering::Relaxed)),
            f32::from_bits(self.allocated[1].load(Ordering::Relaxed)),
        ]
    }

    fn set_allocated(&self, size: [f32; 2]) {
        self.allocated[0].store(size[0].to_bits(), Ordering::Relaxed);
        self.allocated[1].store(size[1].to_bits(), Ordering::Relaxed);
    }

    /// Records `width` as applied, returning whether it differs from the last.
    fn take_wrap_width_change(&self, width: f32) -> bool {
        let bits = width.to_bits();
        self.applied_wrap_width.swap(bits, Ordering::Relaxed) != bits
    }
}

/// Caret blink timing. Separate from [`TextBoxLive`] because only the system
/// needs it; the builder never reads it.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct CaretPhase {
    since: Option<web_time::Instant>,
}

/// The editor generation last drawn, so an edit invalidates exactly once.
#[derive(Component, Clone, Copy, PartialEq, Debug, Default)]
struct DrawnGeneration(Option<parley::Generation>);

/// Builds the message sent after every edit, from the current text.
///
/// A fn pointer rather than a plain `Msg`: the app almost always wants the new
/// text in the message, and only it knows how to phrase that
/// (`|text| Msg::NoteEdited(text.to_string())`).
///
/// No `PartialEq`: comparing function pointers is not meaningful (identical
/// items are not guaranteed to compare equal after codegen), so this is
/// assigned outright on `patch` rather than compared.
#[derive(Component, Clone, Copy, Debug)]
pub struct OnTextUpdate<Msg: Message>(pub Option<fn(&str) -> Msg>);

/// Builds the message sent when the text is confirmed. See [`OnTextUpdate`]
/// for why this does not derive `PartialEq`.
#[derive(Component, Clone, Copy, Debug)]
pub struct OnTextConfirm<Msg: Message>(pub Option<fn(&str) -> Msg>);

/// Which key press means "confirm".
///
/// A predicate rather than a key enum, because the useful bindings differ by
/// context (plain Enter in a one-line field, Ctrl+Enter or Shift+Enter where
/// Enter must stay available for newlines) and a predicate covers all of them
/// without inventing a chord type. See [`confirm_on_enter`] and
/// [`confirm_on_ctrl_enter`] for the two common ones.
///
/// A press that confirms is consumed: it never also inserts a newline.
#[derive(Component, Clone, Copy)]
pub struct ConfirmKey(pub fn(&KeyInput) -> bool);

/// Confirm on plain Enter. Enter no longer inserts a newline, which is what a
/// single-line-style field wants.
pub fn confirm_on_enter(input: &KeyInput) -> bool {
    matches!(input.logical_key(), LogicalKey::Named(NamedKey::Enter))
        && !input.snapshot.modifiers().control_key()
}

/// Confirm on Ctrl+Enter, leaving plain Enter to insert a newline. The default.
pub fn confirm_on_ctrl_enter(input: &KeyInput) -> bool {
    matches!(input.logical_key(), LogicalKey::Named(NamedKey::Enter))
        && input.snapshot.modifiers().control_key()
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

/// A fixed-size, multi-line, editable text box.
///
/// ```ignore
/// s.leaf(
///     TextBox::new(320.0, 96.0)
///         .value(&model.note)
///         .on_update(|text| Msg::NoteEdited(text.to_string()))
/// );
/// ```
///
/// **Requires [`default_systems`] to be registered** through
/// `UiEcs::with_pre_layout_systems`, or the text never re-lays-out and the
/// caret never blinks — the same footgun `animation::default_systems` has.
pub struct TextBox<Msg: Message> {
    key: Key,
    value: String,
    w: f32,
    h: f32,
    style: TextBoxStyle,
    on_update: Option<fn(&str) -> Msg>,
    on_confirm: Option<fn(&str) -> Msg>,
    confirm_key: fn(&KeyInput) -> bool,
}

impl<Msg: Message> TextBox<Msg> {
    pub fn new(w: f32, h: f32) -> Self {
        Self {
            key: Key::Auto,
            value: String::new(),
            w,
            h,
            style: TextBoxStyle {
                font_size: 16.0,
                color: [0.92, 0.92, 0.95, 1.0],
                background_color: [0.16, 0.16, 0.19, 1.0],
                border_color: [0.35, 0.35, 0.4, 1.0],
                focused_border_color: [0.45, 0.7, 1.0, 1.0],
                selection_color: [0.25, 0.45, 0.75, 1.0],
                caret_color: [0.95, 0.95, 1.0, 1.0],
                border_width: 1.0,
                padding: 6.0,
            },
            on_update: None,
            on_confirm: None,
            confirm_key: confirm_on_ctrl_enter,
        }
    }

    /// The text the app wants shown. Applied **only when it differs from the
    /// value declared last time**, so re-declaring an unchanged value never
    /// discards what the user is typing.
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    /// Message to send after every edit, built from the resulting text.
    pub fn on_update(mut self, build: fn(&str) -> Msg) -> Self {
        self.on_update = Some(build);
        self
    }

    /// Message to send when the user confirms — by [`confirm_key`](Self::confirm_key)
    /// or by focus moving away.
    pub fn on_confirm(mut self, build: fn(&str) -> Msg) -> Self {
        self.on_confirm = Some(build);
        self
    }

    /// Which key press confirms. Defaults to [`confirm_on_ctrl_enter`], since
    /// a multi-line box needs plain Enter for newlines; pass
    /// [`confirm_on_enter`] for a field where Enter should submit instead.
    ///
    /// ```ignore
    /// TextBox::new(240.0, 32.0).confirm_key(text_box::confirm_on_enter)
    /// ```
    pub fn confirm_key(mut self, predicate: fn(&KeyInput) -> bool) -> Self {
        self.confirm_key = predicate;
        self
    }

    pub fn font_size(mut self, font_size: f32) -> Self {
        self.style.font_size = font_size;
        self
    }

    pub fn color(mut self, color: [f32; 4]) -> Self {
        self.style.color = color;
        self
    }

    pub fn background_color(mut self, color: [f32; 4]) -> Self {
        self.style.background_color = color;
        self
    }

    pub fn border_color(mut self, color: [f32; 4]) -> Self {
        self.style.border_color = color;
        self
    }

    pub fn focused_border_color(mut self, color: [f32; 4]) -> Self {
        self.style.focused_border_color = color;
        self
    }

    pub fn selection_color(mut self, color: [f32; 4]) -> Self {
        self.style.selection_color = color;
        self
    }

    pub fn caret_color(mut self, color: [f32; 4]) -> Self {
        self.style.caret_color = color;
        self
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    fn layout(&self) -> TextBoxLayout {
        TextBoxLayout {
            w: self.w,
            h: self.h,
        }
    }

    /// The width parley wraps at: the box minus its border and padding.
    fn wrap_width(&self) -> f32 {
        (self.w - (self.style.border_width + self.style.padding) * 2.0).max(0.0)
    }

    fn new_editor(&self) -> TextEditor {
        let mut editor = PlainEditor::new(self.style.font_size);
        editor.set_text(&self.value);
        editor.set_width(Some(self.wrap_width()));
        apply_style(&mut editor, &self.style);
        TextEditor(Arc::new(Mutex::new(editor)))
    }
}

fn apply_style(editor: &mut PlainEditor<RichTextBrush>, style: &TextBoxStyle) {
    let styles = editor.edit_styles();
    styles.insert(StyleProperty::FontSize(style.font_size));
    styles.insert(StyleProperty::Brush(RichTextBrush(style.color)));
}

impl<Msg: Message> Widget for TextBox<Msg> {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            self.new_editor(),
            DeclaredValue(self.value.clone()),
            self.layout(),
            self.style,
            LayoutDispatch::of::<TextBoxLayout>(),
            TextBoxLive::default(),
            CaretPhase::default(),
            DrawnGeneration::default(),
            (
                // A text box paints its own glyphs, so it has to clip itself,
                // not merely its children.
                Clip,
                Pickable,
                // A text box owns its subtree for focus purposes: decorative
                // children must never take the vertex away from it.
                FocusPolicy::Claim,
                // Instantiated per `Msg` so the handlers can queue messages
                // even though the dispatch itself is a plain fn pointer.
                KeyDispatch::new(on_key::<Msg>),
                ImeDispatch::new(on_ime::<Msg>),
                FocusDispatch::new(on_focus_change::<Msg>),
                PointerDispatch::new(on_pointer),
                ImeCursorArea::default(),
                OnTextUpdate(self.on_update),
                OnTextConfirm(self.on_confirm),
                ConfirmKey(self.confirm_key),
            ),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        let item = text_box_render_item(entity, self.style);
        entity.insert(item);
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        let mut needs_rebuild = false;

        // Only an app-driven *change* of the declared value overwrites the
        // buffer; re-declaring the same string leaves the user's edits alone.
        let declared_changed = entity
            .get::<DeclaredValue>()
            .is_some_and(|declared| declared.0 != self.value);
        if declared_changed {
            if let Some(mut declared) = entity.get_mut::<DeclaredValue>() {
                declared.0 = self.value.clone();
            }
            with_editor(entity, |editor| editor.set_text(&self.value));
            needs_rebuild = true;
        }

        if let Some(mut l) = entity.get_mut::<TextBoxLayout>() {
            needs_rebuild |= l.set_if_neq(self.layout());
        }

        let style_changed = entity
            .get::<TextBoxStyle>()
            .is_some_and(|current| *current != self.style);
        if style_changed {
            if let Some(mut s) = entity.get_mut::<TextBoxStyle>() {
                *s = self.style;
            }
            let wrap_width = self.wrap_width();
            let style = self.style;
            with_editor(entity, |editor| {
                editor.set_width(Some(wrap_width));
                apply_style(editor, &style);
            });
            needs_rebuild = true;
        }

        // Assigned rather than compared: see `OnTextUpdate`. Nothing observes
        // `Changed` on these, so the redundant write costs nothing.
        if let Some(mut on_update) = entity.get_mut::<OnTextUpdate<Msg>>() {
            on_update.0 = self.on_update;
        }
        if let Some(mut on_confirm) = entity.get_mut::<OnTextConfirm<Msg>>() {
            on_confirm.0 = self.on_confirm;
        }
        if let Some(mut confirm_key) = entity.get_mut::<ConfirmKey>() {
            confirm_key.0 = self.confirm_key;
        }

        if needs_rebuild {
            let item = text_box_render_item(entity, self.style);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}

/// Run `f` against the entity's editor.
fn with_editor<R>(
    entity: &mut EntityWorldMut,
    f: impl FnOnce(&mut PlainEditor<RichTextBrush>) -> R,
) -> Option<R> {
    let editor = entity.get::<TextEditor>()?.0.clone();
    let mut editor = editor.lock();
    Some(f(&mut editor))
}

/// As [`with_editor`], but also supplies parley's font and layout contexts,
/// which every editing operation needs. They live in a world resource while the
/// editor lives on the entity, so this is the one place that brings them
/// together.
fn with_editor_driver<R>(
    entity: &mut EntityWorldMut,
    f: impl FnOnce(&mut parley::PlainEditorDriver<'_, RichTextBrush>) -> R,
) -> Option<R> {
    let editor = entity.get::<TextEditor>()?.0.clone();
    let font_ctx = entity
        .world_scope(|world| world.get_resource_or_insert_with(ParleyFontCtx::new).clone());
    let mut editor = editor.lock();
    let mut font_cx = font_ctx.0.font_cx.lock();
    let mut layout_cx = font_ctx.0.layout_cx.lock();
    let mut driver = editor.driver(&mut font_cx, &mut layout_cx);
    Some(f(&mut driver))
}

// ---------------------------------------------------------------------------
// Input handling — parley is fair game from here down
// ---------------------------------------------------------------------------

fn on_key<Msg: Message>(entity: &mut EntityWorldMut, input: &KeyInput) -> bool {
    let modifiers = input.snapshot.modifiers();
    let shift = modifiers.shift_key();
    let ctrl = modifiers.control_key();

    // While the IME is composing it owns the keyboard; forwarding keys here
    // would double-handle them.
    if with_editor(entity, |editor| editor.is_composing()).unwrap_or(false) {
        return true;
    }

    // Checked before any editing, so a confirm chord never also inserts.
    let confirms = entity
        .get::<ConfirmKey>()
        .is_some_and(|predicate| (predicate.0)(input));
    if confirms {
        emit::<Msg, OnTextConfirm<Msg>>(entity, |c| c.0);
        return true;
    }

    let handled = with_editor_driver(entity, |d| {
        match input.logical_key() {
            LogicalKey::Named(named) => match named {
                NamedKey::Backspace => {
                    if ctrl {
                        d.backdelete_word();
                    } else {
                        d.backdelete();
                    }
                }
                NamedKey::Delete => {
                    if ctrl {
                        d.delete_word();
                    } else {
                        d.delete();
                    }
                }
                NamedKey::ArrowLeft => match (shift, ctrl) {
                    (false, false) => d.move_left(),
                    (false, true) => d.move_word_left(),
                    (true, false) => d.select_left(),
                    (true, true) => d.select_word_left(),
                },
                NamedKey::ArrowRight => match (shift, ctrl) {
                    (false, false) => d.move_right(),
                    (false, true) => d.move_word_right(),
                    (true, false) => d.select_right(),
                    (true, true) => d.select_word_right(),
                },
                NamedKey::ArrowUp => {
                    if shift {
                        d.select_up();
                    } else {
                        d.move_up();
                    }
                }
                NamedKey::ArrowDown => {
                    if shift {
                        d.select_down();
                    } else {
                        d.move_down();
                    }
                }
                NamedKey::Home => match (shift, ctrl) {
                    (false, false) => d.move_to_line_start(),
                    (false, true) => d.move_to_text_start(),
                    (true, false) => d.select_to_line_start(),
                    (true, true) => d.select_to_text_start(),
                },
                NamedKey::End => match (shift, ctrl) {
                    (false, false) => d.move_to_line_end(),
                    (false, true) => d.move_to_text_end(),
                    (true, false) => d.select_to_line_end(),
                    (true, true) => d.select_to_text_end(),
                },
                // Reached only when Enter is not the configured confirm chord
                // (checked above, before any editing).
                NamedKey::Enter => d.insert_or_replace_selection("\n"),
                NamedKey::Space => d.insert_or_replace_selection(" "),
                // Left alone so a future focus-traversal binding can have it.
                NamedKey::Tab => return false,
                _ => return false,
            },
            LogicalKey::Character(text) => {
                if ctrl {
                    if text.eq_ignore_ascii_case("a") {
                        d.select_all();
                    } else {
                        return false;
                    }
                } else {
                    d.insert_or_replace_selection(text);
                }
            }
            _ => return false,
        }
        true
    })
    .unwrap_or(false);

    if handled {
        emit::<Msg, OnTextUpdate<Msg>>(entity, |c| c.0);
    }
    handled
}

fn on_ime<Msg: Message>(entity: &mut EntityWorldMut, event: &ImeEvent) -> bool {
    let edited = with_editor_driver(entity, |d| match event {
        // `Enabled` only announces that composition may begin.
        ImeEvent::Enabled => false,
        ImeEvent::Preedit { text, cursor } => {
            if text.is_empty() {
                d.clear_compose();
            } else {
                d.set_compose(text, *cursor);
            }
            true
        }
        ImeEvent::Commit { text } => {
            // The preedit is still sitting in the buffer, so drop it first —
            // `clear_compose` also puts the caret back where the composition
            // started, which is exactly where the committed text belongs.
            // (`finish_compose` would instead accept the preedit verbatim, but
            // the platform hands us the final text separately.)
            d.clear_compose();
            d.insert_or_replace_selection(text);
            true
        }
        ImeEvent::Disabled => {
            d.clear_compose();
            true
        }
    })
    .unwrap_or(false);

    if edited {
        emit::<Msg, OnTextUpdate<Msg>>(entity, |c| c.0);
    }
    // Claim the whole session regardless: an IME event that reached a text box
    // must not travel any further.
    true
}

/// Place the caret, extend a selection, or select a word/line, from a click
/// inside the box.
///
/// Not generic over `Msg`: moving the caret changes no text, so there is
/// nothing to notify the app about.
fn on_pointer(entity: &mut EntityWorldMut, input: &PointerInput) -> bool {
    let Some(style) = entity.get::<TextBoxStyle>().copied() else {
        return false;
    };
    let Some(live) = entity.get::<TextBoxLive>().cloned() else {
        return false;
    };

    // A wheel scroll is not a caret interaction: leave it unconsumed so it
    // bubbles to whatever scroll container encloses this box. `TextBox`'s own
    // scrolling follows the caret and is driven by editing, not by the wheel.
    if matches!(input.phase, PointerPhase::Scroll { .. }) {
        return false;
    }

    // Into the editor's own coordinates: strip the border and padding, and undo
    // the scroll offset the text is drawn at.
    let inset = style.border_width + style.padding;
    let x = input.local_pos[0] - inset;
    let y = input.local_pos[1] - inset + live.scroll();

    with_editor_driver(entity, |d| match input.phase {
        PointerPhase::Press { count } => match count {
            1 => d.move_to_point(x, y),
            2 => d.select_word_at_point(x, y),
            _ => d.select_line_at_point(x, y),
        },
        PointerPhase::Drag => d.extend_selection_to_point(x, y),
        // Rejected above; this arm only satisfies exhaustiveness.
        PointerPhase::Scroll { .. } => {}
    });

    // A caret move is not a text change, so nothing invalidates on generation.
    if let Some(mut item) = entity.get_mut::<RenderItem>() {
        item.invalidate();
    }
    true
}

fn on_focus_change<Msg: Message>(entity: &mut EntityWorldMut, gained: bool) {
    if let Some(live) = entity.get::<TextBoxLive>().cloned() {
        // Start (or leave) the caret visible so focus reads immediately.
        live.set_caret_visible(true);
    }
    if let Some(mut phase) = entity.get_mut::<CaretPhase>() {
        phase.since = None;
    }
    if gained {
        return;
    }

    // Losing focus abandons any half-finished composition, then confirms.
    with_editor_driver(entity, |d| d.clear_compose());
    emit::<Msg, OnTextConfirm<Msg>>(entity, |c| c.0);
}

/// Queue the message built by handler component `C` from the current text.
///
/// Handlers run behind a non-generic fn pointer, deep inside dispatch, with no
/// access to the model or the reducer — so the message goes into the core's
/// [`MessageQueue`](matcha_ecs::input::MessageQueue) and `UiEcs` redeems it.
fn emit<Msg: Message, C: Component + Clone>(
    entity: &mut EntityWorldMut,
    build: impl FnOnce(&C) -> Option<fn(&str) -> Msg>,
) {
    let Some(handler) = entity.get::<C>().cloned() else {
        return;
    };
    let Some(build_msg) = build(&handler) else {
        return;
    };
    let Some(editor) = entity.get::<TextEditor>().cloned() else {
        return;
    };
    let text = editor.text();
    entity.world_scope(|world| emit_message(world, build_msg(&text)));
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

impl Layout for TextBoxLayout {
    fn measure(&self, _ctx: &mut LayoutCtx, _me: Entity, c: Constraints) -> Measured {
        // Fixed size: the box does not grow with its content. This is what
        // keeps the wrap width knowable before layout runs, instead of the
        // circular "wrap width needs measure, measure needs wrap width".
        Measured::exact([
            self.w.clamp(c.min_width(), c.max_width()),
            self.h.clamp(c.min_height(), c.max_height()),
        ])
    }

    /// Publishes the size layout actually allocated.
    ///
    /// A parent can hand this box **more** than it asked for (a `Column`'s
    /// default `AlignItems::Stretch` widens it to the widest sibling), and the
    /// declared `w` is only an input to `measure`. Wrapping and caret-follow
    /// scrolling must both use the allocated size, or the text wraps at one
    /// width while the box is painted at another. Same side-channel `Text` and
    /// `RichText` use to publish their wrap width from `arrange`.
    ///
    /// A leaf otherwise: decorative children are not supported in v1.
    fn arrange(&self, ctx: &mut LayoutCtx, me: Entity, size: [f32; 2]) {
        if let Some(live) = ctx.world().get::<TextBoxLive>(me) {
            live.set_allocated(size);
        }
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Systems this widget needs, ready for `UiEcs::with_pre_layout_systems`.
///
/// `PreLayout` is the right stage by its own contract — "settle everything
/// layout reads" — since re-shaping the text is exactly that.
pub fn default_systems() -> ScheduleConfigs<ScheduleSystem> {
    (refresh_text_boxes, blink_carets).chain().into_configs()
}

/// Re-lay-out any editor whose content changed, keep the caret scrolled into
/// view, and publish the caret rectangle for the platform IME.
fn refresh_text_boxes(
    mut query: Query<(
        &TextEditor,
        &TextBoxLayout,
        &TextBoxStyle,
        &TextBoxLive,
        &mut DrawnGeneration,
        &mut RenderItem,
        &mut ImeCursorArea,
        Option<&GlobalTransform>,
    )>,
    font_ctx: Option<Res<ParleyFontCtx>>,
) {
    let Some(font_ctx) = font_ctx else {
        // No text box has been built yet, so parley's contexts do not exist.
        return;
    };

    for (editor, layout, style, live, mut drawn, mut item, mut ime_area, transform) in
        query.iter_mut()
    {
        let mut editor = editor.0.lock();
        {
            let mut font_cx = font_ctx.0.font_cx.lock();
            let mut layout_cx = font_ctx.0.layout_cx.lock();
            // A no-op unless something actually dirtied the layout.
            editor.refresh_layout(&mut font_cx, &mut layout_cx);
        }

        let inset = style.border_width + style.padding;

        // Fall back to the declared size until the first arrange has run.
        let allocated = live.allocated();
        let allocated = [
            if allocated[0] > 0.0 { allocated[0] } else { layout.w },
            if allocated[1] > 0.0 { allocated[1] } else { layout.h },
        ];

        // Re-wrap if the parent gave us a different width than we last shaped
        // at. One frame behind a resize (arrange runs after this stage), which
        // converges immediately and matches `RichText`'s existing behaviour.
        let wrap_width = (allocated[0] - inset * 2.0).max(0.0);
        if live.take_wrap_width_change(wrap_width) {
            editor.set_width(Some(wrap_width));
            let mut font_cx = font_ctx.0.font_cx.lock();
            let mut layout_cx = font_ctx.0.layout_cx.lock();
            editor.refresh_layout(&mut font_cx, &mut layout_cx);
        }

        let generation = editor.generation();
        let inner_h = (allocated[1] - inset * 2.0).max(0.0);

        let caret = editor.cursor_geometry(CARET_WIDTH);

        // Keep the caret inside the visible band.
        if let Some(caret) = caret {
            let (top, bottom) = (caret.y0 as f32, caret.y1 as f32);
            let mut scroll = live.scroll();
            if bottom - scroll > inner_h {
                scroll = bottom - inner_h;
            }
            if top < scroll {
                scroll = top;
            }
            if live.set_scroll(scroll.max(0.0)) {
                item.invalidate();
            }
        }

        // Publish the caret in window space for the IME candidate list. Uses
        // the previous frame's transform, so it trails the caret by one frame —
        // imperceptible for candidate placement, and it keeps this free of any
        // ordering constraint against the core's `sync_ime_state`.
        if let (Some(caret), Some(transform)) = (caret, transform) {
            let origin = transform.affine.transform_point(&Point3::origin());
            let x = origin.x + inset + caret.x0 as f32;
            let y = origin.y + inset + caret.y0 as f32 - live.scroll();
            ime_area.set_if_neq(ImeCursorArea([
                x,
                y,
                x + caret.width() as f32,
                y + caret.height() as f32,
            ]));
        }

        if drawn.0 != Some(generation) {
            drawn.0 = Some(generation);
            item.invalidate();
        }
    }
}

/// Advance the caret blink on the focused box, and keep frames coming while it
/// is focused so the blink actually animates.
fn blink_carets(
    mut query: Query<(
        &TextBoxLive,
        &mut CaretPhase,
        &mut RenderItem,
        Option<&Focused>,
    )>,
    frame_time: Res<FrameTime>,
    mut redraw: ResMut<RedrawRequest>,
) {
    for (live, mut phase, mut item, focused) in query.iter_mut() {
        if focused.is_none() {
            // Unfocused boxes draw no caret at all; reset so the next focus
            // starts from a visible one.
            if live.set_caret_visible(true) {
                item.invalidate();
            }
            phase.since = None;
            continue;
        }

        // Without this the blink would stall as soon as the user stops typing.
        redraw.request();

        let since = *phase.since.get_or_insert(frame_time.0);
        if frame_time.0.duration_since(since).as_secs_f32() >= CARET_BLINK_INTERVAL {
            live.set_caret_visible(!live.caret_visible());
            phase.since = Some(frame_time.0);
            item.invalidate();
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn text_box_render_item(entity: &mut EntityWorldMut, style: TextBoxStyle) -> RenderItem {
    let editor = entity
        .get::<TextEditor>()
        .map(|e| e.0.clone())
        .expect("TextEditor is inserted by bundle() before any render item is built");
    let live = entity.get::<TextBoxLive>().cloned().unwrap_or_default();
    let font_ctx = entity
        .world_scope(|world| world.get_resource_or_insert_with(ParleyFontCtx::new).clone());

    RenderItem::new(move |ctx: &RenderCtx| {
        let [w, h] = ctx.size;
        let border = style.border_width;

        let border_color = if ctx.focused {
            style.focused_border_color
        } else {
            style.border_color
        };
        let mut node = solid_rect_node(ctx, w, h, border_color);
        let fill = solid_rect_node(
            ctx,
            (w - border * 2.0).max(0.0),
            (h - border * 2.0).max(0.0),
            style.background_color,
        );
        node.push_child(
            fill,
            Matrix4::new_translation(&Vector3::new(border, border, 0.0)),
        );

        let inset = style.border_width + style.padding;
        let scroll = live.scroll();

        let editor = editor.lock();
        let Some(layout) = editor.try_layout() else {
            return node;
        };

        let place = |x: f32, y: f32| {
            Matrix4::new_translation(&Vector3::new(inset + x, inset + y - scroll, 0.0))
        };

        // Selection sits under the glyphs.
        for (rect, _line) in editor.selection_geometry() {
            let y0 = rect.y0 as f32;
            let Some(tint) = paint_tint_region(ctx, style.selection_color) else {
                continue;
            };
            let selection = RenderNode::new().with_texture(
                tint,
                [rect.width() as f32, rect.height() as f32],
                Matrix4::identity(),
            );
            node.push_child(selection, place(rect.x0 as f32, y0));
        }

        node.push_child(
            draw_parley_layout(&font_ctx, ctx, layout),
            place(0.0, 0.0),
        );

        if ctx.focused && live.caret_visible() {
            if let Some(caret) = editor.cursor_geometry(CARET_WIDTH)
                && let Some(tint) = paint_tint_region(ctx, style.caret_color)
            {
                let caret_node = RenderNode::new().with_texture(
                    tint,
                    [caret.width() as f32, caret.height() as f32],
                    Matrix4::identity(),
                );
                node.push_child(caret_node, place(caret.x0 as f32, caret.y0 as f32));
            }
        }

        node
    })
}
