//! Upload events: the messages on the ingest topic, each an S3 bucket
//! notification as RustFS publishes it. The ingest listener takes them one at
//! a time and acknowledges each once the catalogue has stored it, so a
//! message is redelivered if the asset server stops before that.

use std::future::Future;

use tokio::sync::{mpsc, oneshot};

/// One message from the ingest topic.
pub struct UploadMessage {
    pub payload: Vec<u8>,
    ack: Box<dyn FnOnce() + Send>,
}

impl UploadMessage {
    pub fn new(payload: Vec<u8>, ack: impl FnOnce() + Send + 'static) -> Self {
        UploadMessage {
            payload,
            ack: Box::new(ack),
        }
    }

    /// Tells the source the message is handled and need not come again.
    pub fn ack(self) {
        (self.ack)()
    }
}

pub trait UploadEvents: Send + 'static {
    /// The next message; `None` when the source is closed.
    fn next(&mut self) -> impl Future<Output = Option<UploadMessage>> + Send;
}

/// An event source fed from the same process (tests).
pub struct InMemoryUploadEvents {
    messages: mpsc::UnboundedReceiver<UploadMessage>,
}

/// Feeds an [`InMemoryUploadEvents`].
#[derive(Clone)]
pub struct UploadEventSender {
    messages: mpsc::UnboundedSender<UploadMessage>,
}

impl InMemoryUploadEvents {
    pub fn new() -> (UploadEventSender, InMemoryUploadEvents) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            UploadEventSender { messages: tx },
            InMemoryUploadEvents { messages: rx },
        )
    }
}

impl UploadEvents for InMemoryUploadEvents {
    async fn next(&mut self) -> Option<UploadMessage> {
        self.messages.recv().await
    }
}

impl UploadEventSender {
    /// Sends one message and returns once it has been acknowledged.
    ///
    /// Panics if the listener drops the message without acknowledging it.
    pub async fn send(&self, payload: Vec<u8>) {
        let (acked, ack) = oneshot::channel();
        self.messages
            .send(UploadMessage::new(payload, move || {
                let _ = acked.send(());
            }))
            .expect("the ingest listener is running");
        ack.await
            .expect("the ingest listener dropped the message without acknowledging it");
    }
}
