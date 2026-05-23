use super::Adapter;
use crate::{application::Application, event::EventStateConfig};

/// Native-only constructors and run loop.
impl<App: Application> Adapter<App> {
    /// Builds an adapter on an externally created Tokio runtime.
    pub fn with_tokio_runtime(app: App, runtime: tokio::runtime::Runtime) -> Self {
        Self::with_tokio_runtime_and_event_config(app, runtime, EventStateConfig::default())
    }

    /// Builds an adapter on an externally created Tokio runtime.
    pub fn with_tokio_runtime_and_event_config(
        app: App,
        runtime: tokio::runtime::Runtime,
        event_config: EventStateConfig,
    ) -> Self {
        Self::with_runtime_and_event_config(
            app,
            crate::runtime::Runtime::from_tokio(runtime),
            event_config,
        )
    }

    #[cfg(feature = "winit")]
    pub fn run(self) -> Result<(), winit::error::EventLoopError> {
        crate::winit_interface::run(self)
    }

    #[cfg(feature = "baseview")]
    pub fn run(self) {
        unimplemented!("baseview support is not implemented yet")
    }
}
