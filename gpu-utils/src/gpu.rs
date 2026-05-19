use log::{debug, error, trace, warn};
use parking_lot::RwLock;
use std::sync::Arc;

/// Descriptor used to configure and create a [`Gpu`] instance.
pub struct GpuDescriptor {
    /// Which wgpu backends to enable.
    pub backends: wgpu::Backends,
    /// Power preference for adapter selection.
    pub power_preference: wgpu::PowerPreference,
    /// Features that must be available on the device.
    pub required_features: wgpu::Features,
    /// Limits to request. If `None`, the adapter's default limits are used.
    pub required_limits: Option<wgpu::Limits>,
    /// Preferred surface format for swapchains created with this GPU.
    pub preferred_surface_format: wgpu::TextureFormat,
}

impl Default for GpuDescriptor {
    fn default() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            backends: wgpu::Backends::PRIMARY,
            #[cfg(target_arch = "wasm32")]
            backends: wgpu::Backends::BROWSER_WEBGPU,

            power_preference: wgpu::PowerPreference::LowPower,

            #[cfg(not(target_arch = "wasm32"))]
            required_features: wgpu::Features::PUSH_CONSTANTS
                | wgpu::Features::VERTEX_WRITABLE_STORAGE,
            #[cfg(target_arch = "wasm32")]
            required_features: wgpu::Features::empty(),

            required_limits: None,
            preferred_surface_format: wgpu::TextureFormat::Bgra8UnormSrgb,
        }
    }
}

/// GPU context: wgpu instance, adapter, device and queue.
///
/// # Render / recovery coordination
///
/// - **Render tasks** call [`context()`] to obtain a cloned `(Device, Queue)`.
///   During recovery the internal write lock is held and [`context()`] returns
///   `None`; the task should skip the frame and return immediately.
/// - **Recovery** is performed by calling [`recover()`], which is a blocking
///   function designed to run inside `tokio::task::spawn_blocking`. It holds
///   the internal write lock for the full duration of the device request.
///
/// [`context()`]: Gpu::context
/// [`recover()`]: Gpu::recover
pub struct Gpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    features: wgpu::Features,
    limits: wgpu::Limits,
    preferred_surface_format: wgpu::TextureFormat,

    device_queue: RwLock<(wgpu::Device, wgpu::Queue)>,
}

impl Gpu {
    /// Create a new `Gpu` from a descriptor.
    pub async fn new(desc: GpuDescriptor) -> Result<Self, GpuError> {
        let GpuDescriptor {
            backends,
            power_preference,
            required_features,
            required_limits,
            preferred_surface_format,
        } = desc;

        trace!("Gpu::new: backends={backends:?} power_preference={power_preference:?}");

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });

        trace!("Gpu::new: requesting adapter");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(GpuError::AdapterRequestFailed)?;
        debug!("Gpu::new: adapter: {:#?}", adapter.get_info());

        let adapter_features = adapter.features();
        if !adapter_features.contains(required_features) {
            warn!(
                "Gpu::new: adapter missing required features \
                required={required_features:?} available={adapter_features:?}"
            );
            return Err(GpuError::AdapterFeatureUnsupported);
        }

        let limits = required_limits.unwrap_or_else(|| adapter.limits());
        let features = required_features;

        trace!("Gpu::new: requesting device");
        let (device, queue) = Self::request_device(&adapter, features, &limits)
            .await
            .map_err(GpuError::DeviceRequestFailed)?;

        let device_queue = RwLock::new((device, queue));

        debug!("Gpu::new: ready");
        Ok(Self {
            instance,
            adapter,
            features,
            limits,
            preferred_surface_format,
            device_queue,
        })
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Clone and return the current device and queue.
    ///
    /// Returns `None` if a write lock is currently held (i.e. recovery is in
    /// progress). Render tasks should treat `None` as a signal to skip the
    /// frame and return immediately — this avoids blocking a tokio thread on
    /// a potentially long-running recovery operation.
    pub fn context(&self) -> Option<(wgpu::Device, wgpu::Queue)> {
        let guard = self.device_queue.try_read()?;
        Some((guard.0.clone(), guard.1.clone()))
    }

    /// Reference to the wgpu instance (needed for surface creation).
    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    /// Reference to the chosen adapter.
    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    /// Features that were requested at creation time.
    pub fn features(&self) -> wgpu::Features {
        self.features
    }

    /// Limits that were requested at creation time.
    pub fn limits(&self) -> &wgpu::Limits {
        &self.limits
    }

    /// Preferred surface format stored in the original descriptor.
    pub fn preferred_surface_format(&self) -> wgpu::TextureFormat {
        self.preferred_surface_format
    }

    // -----------------------------------------------------------------------
    // Recovery
    // -----------------------------------------------------------------------

    /// Request a fresh device and queue from the existing adapter and replace
    /// the current ones.
    ///
    /// This is a **blocking** function intended to be called from
    /// `tokio::task::spawn_blocking`. It holds the internal write lock for the
    /// entire duration of the device request, so concurrent `context()` calls
    /// will return `None` (recovery in progress) until this returns.
    ///
    /// Using `spawn_blocking` at the call site also acts as the single-recovery
    /// guard: the caller is responsible for not invoking this concurrently
    /// (e.g. by checking a flag before spawning, or via an outer `Mutex`).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn recover(&self) -> Result<(wgpu::Device, wgpu::Queue), GpuError> {
        debug!("Gpu::recover: requesting new device");
        let mut device_queue = self.device_queue.write();
        let (device, queue) = futures::executor::block_on(Self::request_device(
            &self.adapter,
            self.features,
            &self.limits,
        ))
        .map_err(GpuError::RecoveryFailed)?;
        *device_queue = (device.clone(), queue.clone());
        debug!("Gpu::recover: done");
        Ok((device, queue))
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    async fn request_device(
        adapter: &wgpu::Adapter,
        features: wgpu::Features,
        limits: &wgpu::Limits,
    ) -> Result<(wgpu::Device, wgpu::Queue), wgpu::RequestDeviceError> {
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: features,
                required_limits: limits.clone(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await?;

        device.on_uncaptured_error(Arc::new(|err| {
            error!("gpu-utils: uncaptured wgpu error: {err:?}");
        }));

        Ok((device, queue))
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(thiserror::Error, Debug)]
pub enum GpuError {
    #[error("Failed to request adapter: {0}")]
    AdapterRequestFailed(wgpu::RequestAdapterError),
    #[error("Adapter does not support required features")]
    AdapterFeatureUnsupported,
    #[error("Failed to request device: {0}")]
    DeviceRequestFailed(wgpu::RequestDeviceError),
    #[error("Device recovery failed: {0}")]
    RecoveryFailed(wgpu::RequestDeviceError),
}
