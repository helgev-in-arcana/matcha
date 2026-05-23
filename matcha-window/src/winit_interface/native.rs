use super::common::{WinitInterface, WinitUserMessage};
use crate::{adapter::Adapter, application::Application};

pub(crate) fn run<App: Application>(
    adapter: Adapter<App>,
) -> Result<(), winit::error::EventLoopError> {
    let event_loop =
        winit::event_loop::EventLoop::<WinitUserMessage<App>>::with_user_event().build()?;

    let event_loop_proxy = event_loop.create_proxy();

    let mut interface = WinitInterface {
        adapter,
        event_loop_proxy,
    };
    event_loop.run_app(&mut interface)
}
