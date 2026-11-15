//! Each component owns its state and receives messages through its own typed channel.
//! Use a [`ComponentPod`] as the root [`Model`] or keep one in a parent and call
//! its [`view`](Model::view) wherever the child should appear.

use std::num::NonZeroUsize;

pub use async_channel::TrySendError as TryEmitError;
use async_channel::{Receiver, Sender};

use crate::app::{Model, UpdateView};
use crate::ui_tree::widget::View;

/// A stateful part of the UI, driven when its parent evaluates its view.
pub trait Component {
    type Message: utils::MaybeSend + 'static;

    /// Called once, the first time this component is evaluated. A task started
    /// here can keep a clone of `events` to deliver its result.
    fn init(&mut self, _events: &EventSender<Self::Message>) {}

    /// Apply one queued message before building the view. Messages emitted to
    /// this component from this method are processed on its next evaluation.
    fn update(&mut self, message: Self::Message, events: &EventSender<Self::Message>);

    /// Pass `update_view` to children and move `events` into widget callbacks.
    fn view(&mut self, update_view: &UpdateView, events: EventSender<Self::Message>) -> impl View;
}

/// The component was dropped before it could accept an asynchronous send.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum EmitError<M> {
    #[error("the component's message channel is closed")]
    Closed(M),
}

/// Sends messages to one component instance from widget callbacks or tasks.
pub struct EventSender<M> {
    sender: Sender<M>,
    update_view: UpdateView,
}

impl<M> Clone for EventSender<M> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            update_view: self.update_view.clone(),
        }
    }
}

impl<M> EventSender<M> {
    /// Send from a widget callback without waiting. A full queue or a dropped
    /// pod returns the original message to the caller.
    pub fn try_emit(&self, message: M) -> Result<(), TryEmitError<M>> {
        self.sender.try_send(message)?;
        self.update_view.request();
        Ok(())
    }

    /// Wait for room in the queue without blocking the calling thread. A full
    /// queue on a hidden pod can stay full until its parent evaluates it again.
    /// Dropping the pod wakes waiting senders and returns their messages.
    pub async fn emit_async(&self, message: M) -> Result<(), EmitError<M>> {
        self.sender
            .send(message)
            .await
            .map_err(|error| EmitError::Closed(error.0))?;
        self.update_view.request();
        Ok(())
    }
}

/// Owns the component across view evaluations. When a parent retains a pod but
/// omits it from its view, messages stay queued until it next evaluates the pod.
pub struct ComponentPod<C: Component> {
    component: C,
    sender: Sender<C::Message>,
    receiver: Receiver<C::Message>,
    initialized: bool,
}

impl<C: Component> ComponentPod<C> {
    /// Create a pod with room for 16 queued messages. Initialization runs on its first view.
    pub fn new(component: C) -> Self {
        const DEFAULT_CAPACITY: NonZeroUsize = NonZeroUsize::new(16).expect("16 is nonzero");
        Self::with_capacity(component, DEFAULT_CAPACITY)
    }

    /// Create a pod with the given message capacity.
    pub fn with_capacity(component: C, capacity: NonZeroUsize) -> Self {
        let (sender, receiver) = async_channel::bounded(capacity.get());
        Self {
            component,
            sender,
            receiver,
            initialized: false,
        }
    }

    fn apply_messages(&mut self, events: &EventSender<C::Message>) {
        // This pod is the sole receiver. Later sends follow this batch in FIFO order.
        let count = self.receiver.len();
        for _ in 0..count {
            let Ok(message) = self.receiver.try_recv() else {
                break;
            };
            self.component.update(message, events);
        }
    }
}

impl<C: Component> Model for ComponentPod<C> {
    fn view(&mut self, update_view: &UpdateView) -> impl View {
        let events = EventSender {
            sender: self.sender.clone(),
            update_view: update_view.clone(),
        };
        if self.initialized {
            self.apply_messages(&events);
        } else {
            self.initialized = true;
            self.component.init(&events);
        }
        self.component.view(update_view, events)
    }
}

impl<C: Component> Drop for ComponentPod<C> {
    fn drop(&mut self) {
        // Senders can outlive the pod. Release buffered messages while we still own the receiver.
        self.receiver.close();
        while self.receiver.try_recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests;
