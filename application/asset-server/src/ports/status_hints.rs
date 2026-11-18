//! Status hints: the transcoder's status messages on `dodoglass/status`.
//! They are best-effort (dropped when the transcoder's queue is full, missed
//! while the asset server is down), so they only trigger an early check; the
//! output marker still decides Ready (ADR 0001).

use std::future::Future;

use tokio::sync::{mpsc, oneshot};

/// One message from the status topic.
pub struct StatusHint {
    pub payload: Vec<u8>,
    handled: Box<dyn FnOnce() + Send>,
}

impl StatusHint {
    pub fn new(payload: Vec<u8>, handled: impl FnOnce() + Send + 'static) -> Self {
        StatusHint {
            payload,
            handled: Box::new(handled),
        }
    }

    /// Tells the source the message has been handled (tests wait for this).
    pub fn handled(self) {
        (self.handled)()
    }
}

pub trait StatusHints: Send + 'static {
    /// The next message; `None` when the source is closed.
    fn next(&mut self) -> impl Future<Output = Option<StatusHint>> + Send;
}

/// A status source fed from the same process (tests).
pub struct InMemoryStatusHints {
    messages: mpsc::UnboundedReceiver<StatusHint>,
}

/// Feeds an [`InMemoryStatusHints`].
#[derive(Clone)]
pub struct StatusHintSender {
    messages: mpsc::UnboundedSender<StatusHint>,
}

impl InMemoryStatusHints {
    pub fn new() -> (StatusHintSender, InMemoryStatusHints) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            StatusHintSender { messages: tx },
            InMemoryStatusHints { messages: rx },
        )
    }
}

impl StatusHints for InMemoryStatusHints {
    async fn next(&mut self) -> Option<StatusHint> {
        self.messages.recv().await
    }
}

impl StatusHintSender {
    /// Sends one message and returns once the listener has handled it.
    pub async fn send(&self, payload: Vec<u8>) {
        let (done, handled) = oneshot::channel();
        self.messages
            .send(StatusHint::new(payload, move || {
                let _ = done.send(());
            }))
            .expect("the status listener is running");
        handled
            .await
            .expect("the status listener dropped the message without handling it");
    }
}
