pub struct App {
    tree: crate::ui_tree::UiTree,
    // Borrow into Context for tree operations; widgets never own this registry.
    windows: crate::window::WindowRegistry,
}

impl App {
    // The application's view-update path lends services only for this operation.
    // Application lifecycle/event routing is still a placeholder below.
    fn update_view<V: crate::widget::View>(&mut self, view: &V) {
        let context = self
            .tree
            .context()
            .clone()
            .with_window_registry(&self.windows);
        self.tree.update_with_context(view, context);
    }
}

impl matcha_window::application::Application for App {
    type Command = ();

    // ------------------
    // Application Events
    // ------------------

    fn init(
        &mut self,
        proxy: Box<dyn matcha_window::adapter::EventLoopProxy<Self>>,
        event_loop: &impl matcha_window::adapter::EventLoop,
    ) {
        todo!()
    }

    fn resumed(&mut self, event_loop: &impl matcha_window::adapter::EventLoop) {
        todo!()
    }

    fn create_surface(&mut self, event_loop: &impl matcha_window::adapter::EventLoop) {
        todo!()
    }

    fn destroy_surface(&mut self, event_loop: &impl matcha_window::adapter::EventLoop) {
        todo!()
    }

    fn suspended(&mut self, event_loop: &impl matcha_window::adapter::EventLoop) {
        todo!()
    }

    fn exiting(&mut self, event_loop: &impl matcha_window::adapter::EventLoop) {
        let _ = event_loop;
        let context = self
            .tree
            .context()
            .clone()
            .with_window_registry(&self.windows);
        self.tree.clear_with_context(context);
    }

    // fn resume_time_reached(
    //     &mut self,
    //     event_loop: &impl matcha_window::adapter::EventLoop,
    //     start: web_time::Instant,
    //     requested_resume: web_time::Instant,
    // ) {
    //     let _ = event_loop;
    //     let _ = start;
    //     let _ = requested_resume;
    // }

    // fn wait_cancelled(
    //     &mut self,
    //     event_loop: &impl matcha_window::adapter::EventLoop,
    //     start: web_time::Instant,
    //     requested_resume: Option<web_time::Instant>,
    // ) {
    //     let _ = event_loop;
    //     let _ = start;
    //     let _ = requested_resume;
    // }

    // fn about_to_wait(&mut self, event_loop: &impl matcha_window::adapter::EventLoop) {
    //     let _ = event_loop;
    // }

    // fn memory_warning(&mut self, event_loop: &impl matcha_window::adapter::EventLoop) {
    //     let _ = event_loop;
    // }

    // --------
    // UI Event
    // --------

    fn window_event(
        &mut self,
        event_loop: &impl matcha_window::adapter::EventLoop,
        window_id: matcha_window::window::WindowId,
        event: matcha_window::event::window_event::WindowEvent,
    ) {
        todo!()
    }

    fn window_destroyed(
        &mut self,
        event_loop: &impl matcha_window::adapter::EventLoop,
        window_id: matcha_window::window::WindowId,
    ) {
        todo!()
    }

    fn device_event(
        &mut self,
        event_loop: &impl matcha_window::adapter::EventLoop,
        window_id: matcha_window::window::WindowId,
        event: matcha_window::event::device_event::DeviceEvent,
    ) {
        todo!()
    }

    // fn raw_device_event(
    //     &mut self,
    //     event_loop: &impl matcha_window::adapter::EventLoop,
    //     raw_device_id: matcha_window::event::raw_device_event::RawDeviceId,
    //     raw_event: matcha_window::event::raw_device_event::RawDeviceEvent,
    // ) {
    //     let _ = event_loop;
    //     let _ = raw_device_id;
    //     let _ = raw_event;
    // }

    // fn poll(&mut self, event_loop: &impl matcha_window::adapter::EventLoop) {
    //     let _ = event_loop;
    // }

    // ---------------
    // Message passing
    // ---------------

    fn ui_command(
        &mut self,
        event_loop: &impl matcha_window::adapter::EventLoop,
        command: Self::Command,
    ) {
        todo!()
    }

    // ---------
    // Rendering
    // ---------

    fn render(&mut self, window_id: matcha_window::window::WindowId) {
        todo!()
    }
}
