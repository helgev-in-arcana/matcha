//! Numerical comparison of two saved renderer outputs, with no image editing.
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 3, "usage: image_diff BASELINE.png ACTUAL.png");
    let a = image::open(&args[1]).expect("baseline").to_rgba8();
    let b = image::open(&args[2]).expect("actual").to_rgba8();
    assert_eq!(a.dimensions(), b.dimensions(), "same viewport");
    let mut changed = 0u64;
    let mut max = 0u8;
    let mut total = 0u64;
    for (a, b) in a.pixels().zip(b.pixels()) {
        if a != b {
            changed += 1;
        }
        for (x, y) in a.0.into_iter().zip(b.0) {
            let d = x.abs_diff(y);
            max = max.max(d);
            total += u64::from(d);
        }
    }
    println!(
        "{}x{}: changed {changed}/{} pixels, maximum channel delta {max}, mean channel delta {:.6}",
        a.width(),
        a.height(),
        u64::from(a.width()) * u64::from(a.height()),
        total as f64 / (a.width() as f64 * a.height() as f64 * 4.)
    );
}
