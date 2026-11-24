//! The MQTT (version 5) connection under the marker events
//! ([`super::mqtt_markers`]): connecting, subscribing again after every
//! reconnect, and handing each message on.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rumqttc::Outgoing;
use rumqttc::v5::mqttbytes::QoS;
use rumqttc::v5::mqttbytes::v5::{Packet, Publish};
use rumqttc::v5::{AsyncClient, Event, EventLoop, MqttOptions};
use tracing::{info, warn};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// A connected client with the subscription it keeps.
pub(super) struct Subscribed {
    pub client: AsyncClient,
    eventloop: EventLoop,
    subscription: String,
}

/// Connects to `broker` (failing if it does not answer within 10 s) with a
/// clean session, and subscribes to `subscription`. `manual_acks`: messages
/// are acknowledged by the caller, not on arrival.
pub(super) async fn connect(
    broker: &str,
    client_id: &str,
    subscription: String,
    request_capacity: usize,
    manual_acks: bool,
) -> Result<Subscribed> {
    let (host, port) = parse_broker(broker)?;
    let mut options = MqttOptions::new(client_id, host, port);
    options
        .set_keep_alive(Duration::from_secs(30))
        .set_clean_start(true)
        .set_manual_acks(manual_acks);
    let (client, mut eventloop) = AsyncClient::new(options, request_capacity);

    let connected = async {
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => return Ok(()),
                Ok(_) => {}
                Err(e) => return Err(e),
            }
        }
    };
    tokio::time::timeout(CONNECT_TIMEOUT, connected)
        .await
        .map_err(|_| anyhow!("no answer from the MQTT broker {broker} within 10 s"))?
        .with_context(|| format!("cannot connect to the MQTT broker {broker}"))?;
    subscribe(&client, &subscription);
    info!(%broker, %subscription, "connected to MQTT broker");
    Ok(Subscribed {
        client,
        eventloop,
        subscription,
    })
}

impl Subscribed {
    /// Keeps the connection up: reconnects after an error, subscribes again
    /// after each reconnect (a clean session forgets the subscription) and
    /// calls `reconnected`, and hands each message to `received` until it
    /// returns `false` (its listener stopped).
    pub async fn run(
        mut self,
        mut reconnected: impl FnMut(),
        mut received: impl FnMut(Publish) -> bool,
    ) {
        loop {
            match self.eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    info!(subscription = %self.subscription, "reconnected to MQTT broker");
                    reconnected();
                    subscribe(&self.client, &self.subscription);
                }
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    if !received(publish) {
                        break;
                    }
                }
                Ok(Event::Outgoing(Outgoing::Disconnect)) => break,
                Ok(_) => {}
                Err(e) => {
                    warn!(subscription = %self.subscription, error = %e, "MQTT connection lost, reconnecting");
                    tokio::time::sleep(RECONNECT_DELAY).await;
                }
            }
        }
    }
}

/// Non-blocking on purpose: this runs inside the loop that drains rumqttc's
/// request queue, so waiting for space could deadlock.
fn subscribe(client: &AsyncClient, subscription: &str) {
    if let Err(e) = client.try_subscribe(subscription, QoS::AtLeastOnce) {
        warn!(%subscription, error = %e, "could not subscribe");
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
