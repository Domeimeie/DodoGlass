//! Upload events over MQTT (version 5): the bucket notifications RustFS
//! publishes on the ingest topic.
//!
//! The asset server subscribes as its own shared-subscription group
//! (`$share/assets/dodoglass/ingest`), so it gets every upload independently
//! of the transcoders' group. Messages are acknowledged by hand, only after
//! the catalogue has stored them.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rumqttc::Outgoing;
use rumqttc::v5::mqttbytes::QoS;
use rumqttc::v5::mqttbytes::v5::{Packet, Publish};
use rumqttc::v5::{AsyncClient, Event, EventLoop, MqttOptions};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::upload_events::{UploadEvents, UploadMessage};

const REQUEST_CAPACITY: usize = 64;
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

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
        let (host, port) = parse_broker(&settings.broker)?;
        let mut options = MqttOptions::new(&settings.client_id, host, port);
        options
            .set_keep_alive(Duration::from_secs(30))
            .set_clean_start(true)
            .set_manual_acks(true);
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
        let background = Background {
            subscription: format!("$share/{}/{}", settings.group, settings.topic),
            client,
            messages: tx,
            connection: Arc::new(AtomicU64::new(1)),
        };
        background.subscribe();
        info!(broker = %broker, subscription = %background.subscription, "connected to MQTT broker");
        tokio::spawn(background.run(eventloop));
        Ok(MqttUploadEvents { messages: rx })
    }
}

impl UploadEvents for MqttUploadEvents {
    async fn next(&mut self) -> Option<UploadMessage> {
        self.messages.recv().await
    }
}

struct Background {
    subscription: String,
    client: AsyncClient,
    messages: mpsc::UnboundedSender<UploadMessage>,
    /// Counts connections: an ack is only valid on the connection its
    /// message came in on (packet ids start over after a reconnect).
    connection: Arc<AtomicU64>,
}

impl Background {
    async fn run(self, mut eventloop: EventLoop) {
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    // clean_start: a new session, so subscribe again.
                    self.connection.fetch_add(1, Ordering::Relaxed);
                    info!("reconnected to MQTT broker");
                    self.subscribe();
                }
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    if self.messages.send(self.message(publish)).is_err() {
                        break; // the listener has stopped
                    }
                }
                Ok(Event::Outgoing(Outgoing::Disconnect)) => break,
                Ok(_) => {}
                Err(e) => {
                    warn!(error = %e, "MQTT connection lost, reconnecting");
                    tokio::time::sleep(RECONNECT_DELAY).await;
                }
            }
        }
    }

    /// Non-blocking on purpose: this runs inside the loop that drains
    /// rumqttc's request queue, so waiting for space could deadlock.
    fn subscribe(&self) {
        if let Err(e) = self
            .client
            .try_subscribe(&self.subscription, QoS::AtLeastOnce)
        {
            warn!(error = %e, "could not subscribe");
        }
    }

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

/// `mqtt://host:port` → (host, port).
fn parse_broker(url: &str) -> Result<(String, u16)> {
    let rest = url
        .strip_prefix("mqtt://")
        .or_else(|| url.strip_prefix("tcp://"))
        .with_context(|| format!("MQTT broker must look like mqtt://host:port, got {url:?}"))?;
    let rest = rest.trim_end_matches('/');
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.parse()
                .with_context(|| format!("invalid port in {url:?}"))?,
        ),
        None => (rest, 1883),
    };
    if host.is_empty() {
        bail!("MQTT broker address {url:?} has no host");
    }
    Ok((host.to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_broker_addresses() {
        assert_eq!(
            parse_broker("mqtt://localhost:1883").unwrap(),
            ("localhost".into(), 1883)
        );
        assert_eq!(
            parse_broker("tcp://mosquitto").unwrap(),
            ("mosquitto".into(), 1883)
        );
        assert!(parse_broker("localhost:1883").is_err());
        assert!(parse_broker("mqtt://:1883").is_err());
    }
}
