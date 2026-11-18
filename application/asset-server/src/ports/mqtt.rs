//! Upload events over MQTT (version 5): the bucket notifications RustFS
//! publishes on the ingest topic.
//!
//! The asset server subscribes as its own shared-subscription group
//! (`$share/assets/dodoglass/ingest`), so it gets every upload independently
//! of the transcoders' group. Messages are acknowledged by hand, only after
//! the catalogue has stored them.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use rumqttc::v5::AsyncClient;
use rumqttc::v5::mqttbytes::v5::Publish;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::mqtt_client;
use super::upload_events::{UploadEvents, UploadMessage};

const REQUEST_CAPACITY: usize = 64;

#[derive(Debug, Clone)]
pub struct MqttSettings {
    /// `mqtt://host:port` (or `tcp://`); the port defaults to 1883.
    pub broker: String,
    /// Topic the bucket events arrive on, e.g. `dodoglass/ingest`.
    pub topic: String,
    /// Shared-subscription group, e.g. `assets`.
    pub group: String,
    /// Must be unique per connection.
    pub client_id: String,
}

pub struct MqttUploadEvents {
    messages: mpsc::UnboundedReceiver<UploadMessage>,
}

impl MqttUploadEvents {
    /// Connects (failing if the broker does not answer within 10 s),
    /// subscribes, and keeps the connection up in the background.
    pub async fn connect(settings: MqttSettings) -> Result<MqttUploadEvents> {
        let subscription = format!("$share/{}/{}", settings.group, settings.topic);
        let connection = mqtt_client::connect(
            &settings.broker,
            &settings.client_id,
            subscription,
            REQUEST_CAPACITY,
            true,
        )
        .await?;

        let (tx, rx) = mpsc::unbounded_channel();
        let acks = Acks {
            client: connection.client.clone(),
            connection: Arc::new(AtomicU64::new(1)),
        };
        let reconnected = {
            let counter = acks.connection.clone();
            move || {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        };
        tokio::spawn(connection.run(reconnected, move |publish| {
            tx.send(acks.message(publish)).is_ok()
        }));
        Ok(MqttUploadEvents { messages: rx })
    }
}

impl UploadEvents for MqttUploadEvents {
    async fn next(&mut self) -> Option<UploadMessage> {
        self.messages.recv().await
    }
}

struct Acks {
    client: AsyncClient,
    /// Counts connections: an ack is only valid on the connection its
    /// message came in on (packet ids start over after a reconnect).
    connection: Arc<AtomicU64>,
}

impl Acks {
    fn message(&self, publish: Publish) -> UploadMessage {
        let client = self.client.clone();
        let connection = self.connection.clone();
        let received_on = connection.load(Ordering::Relaxed);
        let payload = publish.payload.to_vec();
        UploadMessage::new(payload, move || {
            if connection.load(Ordering::Relaxed) == received_on {
                if let Err(e) = client.try_ack(&publish) {
                    warn!(error = %e, "could not acknowledge MQTT message");
                }
            } else {
                debug!("not acknowledging: message came in on an earlier connection");
            }
        })
    }
}
