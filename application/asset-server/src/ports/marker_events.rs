//! Marker events: the messages on `dodoglass/final`, each an S3 bucket
//! notification as RustFS publishes it for a new output marker
//! (`<id>/source.json`) in `final` (ADR 0006). The marker listener takes them
//! one at a time and acknowledges each once the catalogue has stored it, so a
//! message is redelivered if the asset server stops before that.

use std::future::Future;

use tokio::sync::{mpsc, oneshot};

/// One message from `dodoglass/final`.
pub struct MarkerMessage {
    pub payload: Vec<u8>,
    ack: Box<dyn FnOnce() + Send>,
}

impl MarkerMessage {
    pub fn new(payload: Vec<u8>, ack: impl FnOnce() + Send + 'static) -> Self {
        MarkerMessage {
            payload,
            ack: Box::new(ack),
        }
    }

    /// Tells the source the message is handled and need not come again.
    pub fn ack(self) {
        (self.ack)()
    }
}

pub trait MarkerEvents: Send + 'static {
    /// The next message; `None` when the source is closed.
    fn next(&mut self) -> impl Future<Output = Option<MarkerMessage>> + Send;
}

/// An event source fed from the same process (tests).
pub struct InMemoryMarkerEvents {
    messages: mpsc::UnboundedReceiver<MarkerMessage>,
}

/// Feeds an [`InMemoryMarkerEvents`].
#[derive(Clone)]
pub struct MarkerEventSender {
    messages: mpsc::UnboundedSender<MarkerMessage>,
}

impl InMemoryMarkerEvents {
    pub fn new() -> (MarkerEventSender, InMemoryMarkerEvents) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            MarkerEventSender { messages: tx },
            InMemoryMarkerEvents { messages: rx },
        )
    }
}

impl MarkerEvents for InMemoryMarkerEvents {
    async fn next(&mut self) -> Option<MarkerMessage> {
        self.messages.recv().await
    }
}

impl MarkerEventSender {
    /// Sends one message and returns once it has been acknowledged.
    ///
    /// Panics if the listener drops the message without acknowledging it.
    pub async fn send(&self, payload: Vec<u8>) {
        let (acked, ack) = oneshot::channel();
        self.messages
            .send(MarkerMessage::new(payload, move || {
                let _ = acked.send(());
            }))
            .expect("the marker listener is running");
        ack.await
            .expect("the marker listener dropped the message without acknowledging it");
    }
}
