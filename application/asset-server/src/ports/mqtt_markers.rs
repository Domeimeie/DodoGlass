//! Marker events over MQTT (version 5): the bucket notifications RustFS
//! publishes on `dodoglass/final` for new output markers.
//!
//! The asset server subscribes as its own shared-subscription group
//! (`$share/assets/dodoglass/final`), so that a second asset server would not
//! get each marker twice; the transcoders do not subscribe to the topic.
//! Messages are acknowledged by hand, only after the catalogue has stored them.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use rumqttc::v5::AsyncClient;
use rumqttc::v5::mqttbytes::v5::Publish;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::marker_events::{MarkerEvents, MarkerMessage};
use super::mqtt_client;

const REQUEST_CAPACITY: usize = 64;

#[derive(Debug, Clone)]
pub struct MqttSettings {
    /// `mqtt://host:port` (or `tcp://`); the port defaults to 1883.
    pub broker: String,
    /// Topic the bucket events arrive on, e.g. `dodoglass/final`.
    pub topic: String,
    /// Shared-subscription group, e.g. `assets`.
    pub group: String,
    /// Must be unique per connection.
    pub client_id: String,
}

pub struct MqttMarkerEvents {
    messages: mpsc::UnboundedReceiver<MarkerMessage>,
}

impl MqttMarkerEvents {
    /// Connects (failing if the broker does not answer within 10 s),
    /// subscribes, and keeps the connection up in the background.
    pub async fn connect(settings: MqttSettings) -> Result<MqttMarkerEvents> {
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
        Ok(MqttMarkerEvents { messages: rx })
    }
}

impl MarkerEvents for MqttMarkerEvents {
    async fn next(&mut self) -> Option<MarkerMessage> {
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
    fn message(&self, publish: Publish) -> MarkerMessage {
        let client = self.client.clone();
        let connection = self.connection.clone();
        let received_on = connection.load(Ordering::Relaxed);
        let payload = publish.payload.to_vec();
        MarkerMessage::new(payload, move || {
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
