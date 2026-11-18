//! Status hints over MQTT (version 5): the transcoders' status messages on
//! the status topic (`dodoglass/status`).
//!
//! A connection of its own, apart from the upload events: hints are
//! best-effort, so they are acknowledged on arrival and need no shared
//! subscription (only one asset server runs while the database is SQLite).

use anyhow::Result;
use tokio::sync::mpsc;

use super::mqtt_client;
use super::status_hints::{StatusHint, StatusHints};

const REQUEST_CAPACITY: usize = 16;

#[derive(Debug, Clone)]
pub struct MqttStatusSettings {
    /// `mqtt://host:port` (or `tcp://`); the port defaults to 1883.
    pub broker: String,
    /// The transcoders' status topic, e.g. `dodoglass/status`.
    pub topic: String,
    /// Must be unique per connection.
    pub client_id: String,
}

pub struct MqttStatusHints {
    messages: mpsc::UnboundedReceiver<StatusHint>,
}

impl MqttStatusHints {
    /// Connects (failing if the broker does not answer within 10 s),
    /// subscribes, and keeps the connection up in the background.
    pub async fn connect(settings: MqttStatusSettings) -> Result<MqttStatusHints> {
        let connection = mqtt_client::connect(
            &settings.broker,
            &settings.client_id,
            settings.topic,
            REQUEST_CAPACITY,
            false,
        )
        .await?;
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(connection.run(
            || {},
            move |publish| {
                tx.send(StatusHint::new(publish.payload.to_vec(), || {}))
                    .is_ok()
            },
        ));
        Ok(MqttStatusHints { messages: rx })
    }
}

impl StatusHints for MqttStatusHints {
    async fn next(&mut self) -> Option<StatusHint> {
        self.messages.recv().await
    }
}
