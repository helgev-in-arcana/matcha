use super::Adapter;
use crate::application::Application;

/// Native-only constructors and run loop.
impl<App: Application> Adapter<App> {
    #[cfg(feature = "winit")]
    pub fn run(self) -> Result<(), winit::error::EventLoopError> {
        crate::winit_interface::run(self)
    }

    #[cfg(feature = "baseview")]
    pub fn run(self) {
        unimplemented!("baseview support is not implemented yet")
    }
}
