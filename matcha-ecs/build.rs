// Defines the `web` cfg alias used throughout the crate.
// The alias expansion is kept in sync with the `cfg(...)` expressions in
// this crate's `[target.'cfg(...)']` sections in Cargo.toml.
fn main() {
    cfg_aliases::cfg_aliases! {
        web: { all(target_arch = "wasm32", target_os = "unknown") },
    }
}
