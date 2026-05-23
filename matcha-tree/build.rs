// Defines the `web` cfg alias used throughout the crate.
// Kept in sync with the matching alias in sibling crates' build.rs.
fn main() {
    cfg_aliases::cfg_aliases! {
        web: { all(target_arch = "wasm32", target_os = "unknown") },
    }
}
