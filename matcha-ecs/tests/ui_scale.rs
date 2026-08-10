//! `UiScale`: the display's physical-pixels-per-UI-pixel factor.
//!
//! Windows report their size, and deliver pointer positions, in physical
//! pixels; layout and picking work in UI pixels. At 100% on a desktop those
//! coincide, which is why nothing needed this before — but a HiDPI screen or a
//! browser with `devicePixelRatio != 1` makes them differ, and a UI laid out
//! straight against physical pixels comes out at the wrong size.
//!
//! These are unit tests of the conversion itself. Whether `run_layout` and the
//! pointer path apply it is exercised end-to-end by `tests/headless_app.rs`,
//! which drives the real `UiEcs` through the real `Adapter`.
//!
//! It lives on the window entity rather than in a resource because the factor
//! genuinely differs per window — two displays of different densities give the
//! same app two at once. `resumed` seeds it from the window's own reported
//! scale factor, so an app never supplies it.

use matcha_ecs::components::window::UiScale;

#[test]
fn the_default_is_one_to_one() {
    // Native at 100% must be byte-identical to the behaviour before UiScale
    // existed, so the default has to be an exact identity — not merely close.
    let scale = UiScale::default();
    assert_eq!(scale.0, 1.0);
    assert_eq!(scale.to_ui([800.0, 600.0]), [800.0, 600.0]);
}

#[test]
fn a_scale_of_two_halves_the_ui_size() {
    // A 1600x1200 framebuffer on a dpr-2 display is an 800x600 UI. Getting this
    // backwards is the bug this guards: multiplying instead of dividing gives a
    // UI laid out at twice the space that exists.
    let scale = UiScale(2.0);
    assert_eq!(scale.to_ui([1600.0, 1200.0]), [800.0, 600.0]);
}

#[test]
fn fractional_scales_are_supported() {
    // Windows' 150% and the browser zoom levels are not integers.
    let scale = UiScale(1.5);
    let [w, h] = scale.to_ui([1200.0, 900.0]);
    assert!((w - 800.0).abs() < 1e-3, "width was {w}");
    assert!((h - 600.0).abs() < 1e-3, "height was {h}");
}

#[test]
fn conversion_round_trips_a_pointer_position() {
    // Pointer positions and layout rects must agree, or clicks land somewhere
    // other than where the user aimed. Same conversion, same factor, so a
    // point at the centre of a scaled viewport stays at the centre.
    let scale = UiScale(2.0);
    let viewport_ui = scale.to_ui([1000.0, 800.0]);
    let centre_ui = scale.to_ui([500.0, 400.0]);
    assert_eq!(centre_ui, [viewport_ui[0] / 2.0, viewport_ui[1] / 2.0]);
}
