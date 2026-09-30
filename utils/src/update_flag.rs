use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

pub struct UpdateFlag {
    // default is false
    // when the flag is set to true, then dom update is triggered
    flag: Arc<AtomicBool>,
}

impl Default for UpdateFlag {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateFlag {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Creates a new UpdateFlag that is initially set to true.
    pub fn new_true() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn reset(&self) {
        self.flag.store(false, Ordering::Release);
    }

    pub fn set_true(&self) {
        self.flag.store(true, Ordering::Release);
    }

    pub fn notifier(&self) -> UpdateNotifier {
        UpdateNotifier {
            flag: Arc::downgrade(&self.flag),
        }
    }
}

impl UpdateFlag {
    pub fn is_true(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    pub async fn wait(&self) {
        while !self.is_true() {
            tokio::task::yield_now().await;
        }
    }
}

#[derive(Clone)]
pub struct UpdateNotifier {
    flag: Weak<AtomicBool>,
}

impl UpdateNotifier {
    pub fn notify(&mut self) {
        if let Some(flag) = self.flag.upgrade() {
            flag.store(true, Ordering::Release);
        }
    }
}
