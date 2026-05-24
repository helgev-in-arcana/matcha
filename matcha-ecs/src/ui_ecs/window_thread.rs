use std::thread::JoinHandle;

pub struct WindowThread {
    thread_handle: JoinHandle<()>,
    channel: std::sync::mpsc::SyncSender<Box<dyn FnOnce() + Send>>,
}

impl WindowThread {
    pub fn new() -> Self {
        let (channel, receiver) = std::sync::mpsc::sync_channel::<Box<dyn FnOnce() + Send>>(1);
        let thread_handle = std::thread::spawn(move || {
            loop {
                if let Some(task) = receiver.recv().ok() {
                    task();
                } else {
                    break;
                }
            }
        });
        Self {
            thread_handle,
            channel,
        }
    }

    pub fn try_send<F>(
        &self,
        task: F,
    ) -> Result<(), std::sync::mpsc::TrySendError<Box<dyn FnOnce() + Send>>>
    where
        F: FnOnce() + Send + 'static,
    {
        self.channel.try_send(Box::new(task))
    }
}
