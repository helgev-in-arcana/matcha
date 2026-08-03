//! Errors this crate reports to the application.

/// The event loop stopped for a reason other than the application asking it to.
///
/// Mirrors the shape every desktop backend can report, without naming one:
/// winit's `EventLoopError` maps onto it directly, and baseview — whose
/// `open_blocking` returns `()` and panics instead — simply never produces one.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The platform has no event loop this backend can drive.
    #[error("running an event loop is not supported on this platform")]
    NotSupported,
    /// The OS refused. Carries the backend's message; see
    /// [`WindowSurfaceError::CreateWindow`](crate::window::WindowSurfaceError::CreateWindow)
    /// for why this is a string.
    #[error("os error: {0}")]
    Os(String),
    /// An event loop was already run in this process and cannot be rebuilt.
    #[error("the event loop cannot be created twice")]
    RecreationAttempt,
    /// The application asked to exit with a failing status.
    #[error("application exited with status {0}")]
    ExitFailure(i32),
    /// A backend failure with no portable classification.
    #[error("event loop failed: {0}")]
    Other(String),
}

#[cfg(feature = "winit")]
impl From<winit::error::EventLoopError> for RunError {
    fn from(e: winit::error::EventLoopError) -> Self {
        use winit::error::EventLoopError;
        match e {
            EventLoopError::NotSupported(_) => Self::NotSupported,
            EventLoopError::Os(os) => Self::Os(os.to_string()),
            EventLoopError::RecreationAttempt => Self::RecreationAttempt,
            EventLoopError::ExitFailure(code) => Self::ExitFailure(code),
        }
    }
}
