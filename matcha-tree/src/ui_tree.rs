pub mod component;
pub mod context;
pub mod metrics;
pub(crate) mod runtime;
pub mod sub_widgets;
pub mod widget;
pub mod window;

use dashmap::DashMap;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{
    Arc, OnceLock, Weak,
    atomic::{AtomicBool, Ordering},
};

use matcha_window::adapter::{EventLoop, EventLoopProxy};
use matcha_window::application::Application;
use matcha_window::event::device_event::DeviceEvent;
use matcha_window::event::raw_device_event::{RawDeviceEvent, RawDeviceId};
use matcha_window::event::window_event::WindowEvent;
use matcha_window::window::WindowId;

use component::{Component, ComponentPod};
use context::{AppContext, EventReceiver, EventSender, SharedCtx, UiContext};
use gpu_utils::gpu::{Gpu, GpuDescriptor, GpuError};
use gpu_utils::texture_atlas::atlas_simple::atlas::TextureAtlas;
use runtime::{JoinHandle, Runtime, RuntimeHandle};
use shared_buffer::BufferContext;
use widget::{View, WidgetPod, WidgetUpdateError};
use window::AnyWindowWidgetInstance;

// ----------------------------------------------------------------------------
// UiTree
// ----------------------------------------------------------------------------

pub struct UiTree<C: Component> {
    /// Async runtime owned by this app (tokio on native, browser queue on WASM).
    runtime: Runtime,

    /// GPU device / queue / instance. Wrapped in Arc so render tasks can share it.
    gpu: Arc<Gpu>,

    root: ComponentPod<C>,

    /// Built widget tree.  `None` until the first `create_window` / `buffer_updated`.
    widget_pod: Mutex<Option<WidgetPod>>,

    /// Weak registry keyed by [`WindowId`].
    /// Wrapped in Arc so render tasks can clone a reference.
    window_registry: Arc<DashMap<WindowId, Weak<Mutex<dyn AnyWindowWidgetInstance>>>>,

    event_sender: EventSender,

    /// Receiver end of the backend message channel.
    /// Wrapped in `Mutex<Option<>>` solely to satisfy `Sync` (`UnboundedReceiver: !Sync`).
    /// Extracted once in `init()` via `Mutex::get_mut()` — no runtime locking occurs.
    event_receiver: Mutex<Option<EventReceiver>>,

    /// Handle to the bridge task spawned in `init()`.
    bridge_handle: OnceLock<JoinHandle>,

    /// Shared texture atlas for widget rendering (format: Rgba8UnormSrgb).
    texture_atlas: Arc<TextureAtlas>,

    /// Renderer pipeline. Wrapped in Arc so render tasks can share it.
    core_renderer: Arc<renderer::CoreRenderer>,

    /// Texture atlas for stencils (format: R8Unorm).
    stencil_atlas: Arc<TextureAtlas>,

    /// Flag tracking whether surface creation is currently permitted.
    surface_creation_permitted: AtomicBool,

    /// In-flight per-window render tasks.
    /// Mutex (not DashMap) so we can drain the map in destroy_surface.
    rendering_tasks: Mutex<HashMap<WindowId, JoinHandle>>,
}

// ----------------------------------------------------------------------------
// Construction
// ----------------------------------------------------------------------------

impl<C: Component> UiTree<C> {
    /// Creates a `UiTree` with an already-initialised `Gpu`.
    pub fn new(root: C, gpu: Gpu) -> Self {
        Self::with_runtime(root, gpu, Runtime::new())
    }

    /// Creates a `UiTree` from a `GpuDescriptor`, initialising GPU and runtime internally.
    ///
    /// Not available on WASM: GPU initialisation must be performed in an async
    /// WASM entry point before calling `UiTree::new`.
    #[cfg(not(web))]
    pub fn new_with_descriptor(root: C, desc: GpuDescriptor) -> Result<Self, GpuError> {
        let runtime = Runtime::new();
        let gpu = runtime.block_on(Gpu::new(desc))?;
        Ok(Self::with_runtime(root, gpu, runtime))
    }

    fn with_runtime(root: C, gpu: Gpu, runtime: Runtime) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let (gpu_device, _) = gpu.context().unwrap();
        let texture_atlas = TextureAtlas::new(
            &gpu_device,
            wgpu::Extent3d {
                width: 4096,
                height: 4096,
                depth_or_array_layers: 4,
            },
            wgpu::TextureFormat::Rgba8UnormSrgb,
            TextureAtlas::DEFAULT_MARGIN_PX,
        );
        let stencil_atlas = TextureAtlas::new(
            &gpu_device,
            wgpu::Extent3d {
                width: 4096,
                height: 4096,
                depth_or_array_layers: 4,
            },
            wgpu::TextureFormat::R8Unorm,
            TextureAtlas::DEFAULT_MARGIN_PX,
        );
        let core_renderer = Arc::new(renderer::CoreRenderer::new(&gpu_device));

        Self {
            runtime,
            gpu: Arc::new(gpu),
            root: ComponentPod::new(None, root),
            widget_pod: Mutex::new(None),
            window_registry: Arc::new(DashMap::new()),
            event_sender: EventSender::new(tx),
            event_receiver: Mutex::new(Some(EventReceiver::new(rx))),
            bridge_handle: OnceLock::new(),
            texture_atlas,
            core_renderer,
            stencil_atlas,
            surface_creation_permitted: AtomicBool::new(false),
            rendering_tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Returns a cloned `Arc` to the inner component.
    pub fn component(&self) -> Arc<C> {
        self.root.arc()
    }
}

// ----------------------------------------------------------------------------
// Core logic
// ----------------------------------------------------------------------------

impl<C: Component> UiTree<C> {
    fn runtime_handle(&self) -> RuntimeHandle {
        self.runtime.handle().clone()
    }

    fn run_update(&self, event_loop: &dyn EventLoop) {
        let gpu_instance = self.gpu.instance();
        let (gpu_device, gpu_queue) = self.gpu.context().unwrap();

        let shared = SharedCtx {
            runtime_handle: self.runtime_handle(),
            event_sender: &self.event_sender,
            window_registry: &self.window_registry,
            gpu_instance,
            gpu_device,
            gpu_queue,
            texture_atlas: self.texture_atlas.as_ref(),
            surface_creation_permitted: self.surface_creation_permitted.load(Ordering::SeqCst),
        };
        let ctx = UiContext {
            shared: &shared,
            event_loop: Some(event_loop),
            window: None,
        };

        let view = self.root.view(&ctx);

        let mut widget_pod = self.widget_pod.lock();
        match &mut *widget_pod {
            None => {
                *widget_pod = Some(view.build(&ctx));
            }
            Some(pod) => {
                if let Err(WidgetUpdateError::TypeMismatch) = pod.try_update(&view, &ctx) {
                    *pod = view.build(&ctx);
                }
            }
        }

        self.window_registry
            .retain(|_, weak| weak.strong_count() > 0);
    }

    /// Requests the OS to schedule another redraw for `window_id`.
    /// Called when a render task for the window is still in flight.
    fn request_window_redraw(&self, window_id: WindowId) {
        if let Some(arc) = self
            .window_registry
            .get(&window_id)
            .and_then(|w| w.upgrade())
        {
            arc.lock().request_redraw();
        }
    }
}

// ----------------------------------------------------------------------------
// Application impl
// ----------------------------------------------------------------------------

impl<C: Component> Application for UiTree<C> {
    type Command = TreeAppCommand<C::Message>;

    // -------------------------------------------------------------------------
    // Lifecycle
    // -------------------------------------------------------------------------

    fn init(&mut self, proxy: Box<dyn EventLoopProxy<Self>>, event_loop: &impl EventLoop) {
        // Extract the receiver without locking — safe because `init` has `&mut self`.
        let mut receiver = self
            .event_receiver
            .get_mut()
            .take()
            .expect("UiTree::init called more than once");

        let mut buffer_rx = BufferContext::global().subscribe();

        let handle = self.runtime.handle().spawn(async move {
            loop {
                tokio::select! {
                    result = buffer_rx.changed() => {
                        if result.is_err() {
                            break;
                        }
                        proxy.send_command(TreeAppCommand::BufferUpdated);
                    }
                    msg = receiver.recv() => match msg {
                        Some(boxed) => {
                            if let Ok(m) = boxed.into_any().downcast::<C::Message>() {
                                proxy.send_command(TreeAppCommand::BackendMessage(*m));
                            }
                        }
                        None => break,
                    }
                }
            }
        });

        self.bridge_handle.set(handle).ok();

        let ctx = AppContext {
            runtime_handle: self.runtime_handle(),
            event_sender: &self.event_sender,
            event_loop,
        };
        self.root.init(&ctx);
    }

    fn resumed(&self, event_loop: &impl EventLoop) {
        let ctx = AppContext {
            runtime_handle: self.runtime_handle(),
            event_sender: &self.event_sender,
            event_loop,
        };
        self.root.resumed(&ctx);
    }

    fn create_surface(&self, event_loop: &impl EventLoop) {
        self.surface_creation_permitted
            .store(true, Ordering::SeqCst);

        let gpu_instance = self.gpu.instance();
        let (gpu_device, _) = self.gpu.context().unwrap();

        for entry in self.window_registry.iter() {
            if let Some(arc) = entry.value().upgrade() {
                let mut instance = arc.lock();
                let _ = instance.create_surface(gpu_instance, &gpu_device);
            }
        }

        self.run_update(event_loop);
    }

    fn destroy_surface(&self, _event_loop: &impl EventLoop) {
        self.surface_creation_permitted
            .store(false, Ordering::SeqCst);

        // Abort and join all in-flight render tasks before destroying surfaces.
        let handles: Vec<_> = self.rendering_tasks.lock().drain().map(|(_, h)| h).collect();
        self.runtime.abort_and_join(handles);

        for entry in self.window_registry.iter() {
            if let Some(arc) = entry.value().upgrade() {
                let mut instance = arc.lock();
                instance.destroy_surface();
            }
        }
    }

    fn suspended(&self, event_loop: &impl EventLoop) {
        let ctx = AppContext {
            runtime_handle: self.runtime_handle(),
            event_sender: &self.event_sender,
            event_loop,
        };
        self.root.suspended(&ctx);
    }

    fn exiting(&self, event_loop: &impl EventLoop) {
        let ctx = AppContext {
            runtime_handle: self.runtime_handle(),
            event_sender: &self.event_sender,
            event_loop,
        };
        self.root.exiting(&ctx);
    }

    // -------------------------------------------------------------------------
    // Rendering
    // -------------------------------------------------------------------------

    fn render(&self, window_id: WindowId) {
        // If a render task for this window is still running, request another
        // frame and return — the new frame will be triggered once this task finishes.
        {
            let mut tasks = self.rendering_tasks.lock();
            if let Some(handle) = tasks.get(&window_id) {
                if !handle.is_finished() {
                    drop(tasks);
                    self.request_window_redraw(window_id);
                    return;
                }
            }

            // Clone Arcs to move into the spawned task.
            let gpu = Arc::clone(&self.gpu);
            let core_renderer = Arc::clone(&self.core_renderer);
            let texture_atlas = Arc::clone(&self.texture_atlas);
            let stencil_atlas = Arc::clone(&self.stencil_atlas);
            let window_registry = Arc::clone(&self.window_registry);
            let event_sender = self.event_sender.clone();
            let runtime_handle = self.runtime_handle();
            let surface_creation_permitted =
                self.surface_creation_permitted.load(Ordering::SeqCst);

            let handle = self.runtime.handle().spawn(async move {
                let op_arc = window_registry.get(&window_id).and_then(|w| w.upgrade());
                if let Some(arc) = op_arc {
                    let gpu_instance = gpu.instance();
                    let (gpu_device, gpu_queue) = match gpu.context() {
                        Some(ctx) => ctx,
                        None => return,
                    };
                    let shared = SharedCtx {
                        runtime_handle,
                        event_sender: &event_sender,
                        window_registry: &window_registry,
                        gpu_instance,
                        gpu_device,
                        gpu_queue,
                        texture_atlas: texture_atlas.as_ref(),
                        surface_creation_permitted,
                    };
                    let ctx = UiContext {
                        shared: &shared,
                        event_loop: None,
                        window: None,
                    };
                    let mut instance = arc.lock();
                    instance.render(
                        &core_renderer,
                        &texture_atlas.texture(),
                        &stencil_atlas.texture(),
                        &ctx,
                    );
                }
            });

            tasks.insert(window_id, handle);
        }
    }

    // -------------------------------------------------------------------------
    // Events
    // -------------------------------------------------------------------------

    fn window_event(
        &self,
        _event_loop: &impl EventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
        // TODO
    }

    fn window_destroyed(&self, _event_loop: &impl EventLoop, _window_id: WindowId) {
        // TODO
    }

    fn device_event(
        &self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: DeviceEvent,
    ) {
        let op_arc = self
            .window_registry
            .get(&window_id)
            .and_then(|w| w.upgrade());

        if let Some(arc) = op_arc {
            let gpu_instance = self.gpu.instance();
            let (gpu_device, gpu_queue) = self.gpu.context().unwrap();

            let shared = SharedCtx {
                runtime_handle: self.runtime_handle(),
                event_sender: &self.event_sender,
                window_registry: &self.window_registry,
                gpu_instance,
                gpu_device,
                gpu_queue,
                texture_atlas: self.texture_atlas.as_ref(),
                surface_creation_permitted: self.surface_creation_permitted.load(Ordering::SeqCst),
            };
            let ctx = UiContext {
                shared: &shared,
                event_loop: Some(event_loop),
                window: None,
            };

            let mut instance = arc.lock();
            instance.device_input(&event, &ctx);
        }
    }

    fn raw_device_event(
        &self,
        _event_loop: &impl EventLoop,
        _raw_device_id: RawDeviceId,
        _raw_event: RawDeviceEvent,
    ) {
        // TODO
    }

    // -------------------------------------------------------------------------
    // Ui commands
    // -------------------------------------------------------------------------

    fn ui_command(&self, event_loop: &impl EventLoop, command: Self::Command) {
        match command {
            TreeAppCommand::BufferUpdated => {
                self.run_update(event_loop);
            }
            TreeAppCommand::BackendMessage(msg) => {
                let gpu_instance = self.gpu.instance();
                let (gpu_device, gpu_queue) = self.gpu.context().unwrap();

                let shared = SharedCtx {
                    runtime_handle: self.runtime_handle(),
                    event_sender: &self.event_sender,
                    window_registry: &self.window_registry,
                    gpu_instance,
                    gpu_device,
                    gpu_queue,
                    texture_atlas: self.texture_atlas.as_ref(),
                    surface_creation_permitted: self
                        .surface_creation_permitted
                        .load(Ordering::SeqCst),
                };
                let ctx = UiContext {
                    shared: &shared,
                    event_loop: Some(event_loop),
                    window: None,
                };

                self.root.update(msg, &ctx);
            }
        }
    }
}

pub enum TreeAppCommand<Msg: utils::MaybeSend + 'static> {
    BufferUpdated,
    BackendMessage(Msg),
}
