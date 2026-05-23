use super::Adapter;
use crate::application::Application;

/// Web-only run loop. On wasm the browser owns the event loop, so `run`
/// returns immediately after handing the adapter off to winit.
impl<App: Application> Adapter<App> {
    #[cfg(feature = "winit")]
    pub fn run(self) {
        crate::winit_interface::run(self)
    }
}
