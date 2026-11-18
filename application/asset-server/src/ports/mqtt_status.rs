//! Status hints over MQTT (version 5): the transcoders' status messages on
//! the status topic (`dodoglass/status`).
//!
//! A connection of its own, apart from the upload events: hints are
//! best-effort, so they are acknowledged on arrival and need no shared
//! subscription (only one asset server runs while the database is SQLite).

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use rumqttc::Outgoing;
use rumqttc::v5::mqttbytes::QoS;
use rumqttc::v5::mqttbytes::v5::Packet;
use rumqttc::v5::{AsyncClient, Event, EventLoop, MqttOptions};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::mqtt::parse_broker;
use super::status_hints::{StatusHint, StatusHints};

const REQUEST_CAPACITY: usize = 16;
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

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
        let (host, port) = parse_broker(&settings.broker)?;
        let mut options = MqttOptions::new(&settings.client_id, host, port);
        options
            .set_keep_alive(Duration::from_secs(30))
            .set_clean_start(true);
        let (client, mut eventloop) = AsyncClient::new(options, REQUEST_CAPACITY);

        let connected = async {
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => return Ok(()),
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
            }
        };
        let broker = settings.broker.clone();
        tokio::time::timeout(Duration::from_secs(10), connected)
            .await
            .map_err(|_| anyhow!("no answer from the MQTT broker {broker} within 10 s"))?
            .with_context(|| format!("cannot connect to the MQTT broker {broker}"))?;

        let (tx, rx) = mpsc::unbounded_channel();
        subscribe(&client, &settings.topic);
        info!(broker = %broker, topic = %settings.topic, "subscribed to status hints");
        tokio::spawn(run(client, eventloop, settings.topic, tx));
        Ok(MqttStatusHints { messages: rx })
    }
}

impl StatusHints for MqttStatusHints {
    async fn next(&mut self) -> Option<StatusHint> {
        self.messages.recv().await
    }
}

async fn run(
    client: AsyncClient,
    mut eventloop: EventLoop,
    topic: String,
    messages: mpsc::UnboundedSender<StatusHint>,
) {
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                // clean_start: a new session, so subscribe again.
                info!("status hints: reconnected to MQTT broker");
                subscribe(&client, &topic);
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let hint = StatusHint::new(publish.payload.to_vec(), || {});
                if messages.send(hint).is_err() {
                    break; // the listener has stopped
                }
            }
            Ok(Event::Outgoing(Outgoing::Disconnect)) => break,
            Ok(_) => {}
            Err(e) => {
                warn!(error = %e, "status hints: MQTT connection lost, reconnecting");
                tokio::time::sleep(RECONNECT_DELAY).await;
            }
        }
    }
}

/// Non-blocking, as in [`super::mqtt`]: this runs inside the event loop.
fn subscribe(client: &AsyncClient, topic: &str) {
    if let Err(e) = client.try_subscribe(topic, QoS::AtLeastOnce) {
        warn!(error = %e, "could not subscribe to status hints");
    }
}
