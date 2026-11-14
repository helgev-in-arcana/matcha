//! Synchronous presentation of a window's retained Scene on the application's GPU.
use gpu_utils::gpu::Gpu;
use matcha_window::window::{Window, WindowConfig, WindowError, WindowId};
use plain_renderer::{PlainRenderer, PlainTarget};
use render_interface::TextureRegion;

use crate::ui_tree::{UiTree, context::Context};

pub(super) struct Presentation {
    instance: wgpu::Instance,
    device: wgpu::Device,
    renderer: PlainRenderer,
    surface_config: wgpu::SurfaceConfiguration,
}

impl Presentation {
    pub fn new(gpu: Gpu) -> Self {
        let (device, queue) = gpu.context().expect("new GPU has a device and queue");
        let renderer = PlainRenderer::new(&device, &queue);
        let surface_config = WindowConfig::default().surface_config;
        // Web uses matcha-window's canvas format and its corresponding sRGB render view.
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        let surface_config = wgpu::SurfaceConfiguration {
            format: gpu.preferred_surface_format(),
            ..surface_config
        };
        Self {
            instance: gpu.instance().clone(),
            device,
            renderer,
            surface_config,
        }
    }

    pub fn surface_config(&self) -> &wgpu::SurfaceConfiguration {
        &self.surface_config
    }

    pub fn attach(&self, window: &mut Window) -> Result<(), WindowError> {
        if !window.has_surface() {
            window.create_surface(&self.instance, &self.device)?;
            window.request_redraw();
        }
        Ok(())
    }

    pub fn reconfigure_surface(&self, window: &Window) {
        window.surface().reconfigure(&self.device);
    }

    pub fn render(
        &mut self,
        id: WindowId,
        tree: &mut UiTree,
        ctx: &mut Context<'_>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tree.layout_window(id, ctx);
        let Some(scene) = tree.render_window(id, ctx)? else {
            return Ok(());
        };

        let Some(window) = ctx.window_registry().window(id) else {
            return Ok(());
        };
        if window.inner_size().contains(&0) {
            return Ok(());
        }
        let Some(frame) = window.surface().get_surface_texture(&self.device)? else {
            return Ok(());
        };

        let view = window.surface().create_render_view(&frame.texture);
        let region = TextureRegion::whole(&view, window.render_format())?;
        // UiTree's window viewport and widget coordinates are in physical pixels.
        let logical_size = region.size().map(|pixels| pixels as f32);
        self.renderer.render(
            &scene,
            PlainTarget {
                region,
                logical_size,
                clear: wgpu::Color::TRANSPARENT,
                initial: None,
            },
        )?;
        frame.present();
        Ok(())
    }
}
