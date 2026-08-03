use super::common::{WinitInterface, WinitUserMessage};
use crate::{adapter::Adapter, application::Application};

/// `spawn_app` hands control back to the browser event loop immediately;
/// the adapter is then driven by browser callbacks.
pub(crate) fn run<App: Application>(adapter: Adapter<App>) {
    use winit::platform::web::EventLoopExtWebSys;

    let event_loop = winit::event_loop::EventLoop::<WinitUserMessage<App>>::with_user_event()
        .build()
        .expect("failed to build winit event loop");

    let event_loop_proxy = event_loop.create_proxy();

    let interface = WinitInterface {
        adapter,
        event_loop_proxy,
    };
    event_loop.spawn_app(interface);
}
