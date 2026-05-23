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
        #[cfg(not(web))]
        let backends = wgpu::Backends::PRIMARY;
        #[cfg(web)]
        let backends = wgpu::Backends::BROWSER_WEBGPU;

        #[cfg(not(web))]
        let required_features =
            wgpu::Features::PUSH_CONSTANTS | wgpu::Features::VERTEX_WRITABLE_STORAGE;
        // WebGPU exposes neither feature; wasm renderers need other fallbacks.
        #[cfg(web)]
        let required_features = wgpu::Features::empty();

        Self {
            backends,
            power_preference: wgpu::PowerPreference::LowPower,
            required_features,
            required_limits: None,
            preferred_surface_format: wgpu::TextureFormat::Bgra8UnormSrgb,
        }
    }
}

/// GPU context: wgpu instance, adapter, device and queue.
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
}
