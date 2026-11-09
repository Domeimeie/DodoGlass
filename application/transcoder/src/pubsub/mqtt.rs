//! PubSub over MQTT (version 5). Tasks are the bucket events RustFS publishes
//! when a video is uploaded; status goes to a status topic.
//!
//! ```text
//! $share/<group>/<topic>      ← subscribed: S3 events, each delivered to ONE worker of the group
//! <status topic>              ← every status message, JSON
//! <workers topic>/<worker>    ← retained registration; cleared on exit, by the broker on a crash
//! ```
//!
//! How it fits the pull-style service (`service.rs` takes a slot, then asks for a task):
//!
//! - rumqttc's event loop must be polled all the time (keep-alive pings,
//!   incoming messages, outgoing status), so it runs in its own tokio task and
//!   hands tasks to [`next_task`](PubSub::next_task) through a channel.
//! - **Receive Maximum = slots, and the message is acknowledged only when its
//!   task is finished.** The broker never has more unacknowledged messages
//!   out to a worker than it has slots, so a busy worker is not sent work it
//!   cannot start, and events wait in the broker rather than in the memory of
//!   a worker.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rumqttc::Outgoing;
use rumqttc::v5::mqttbytes::QoS;
use rumqttc::v5::mqttbytes::v5::{LastWill, Packet, Publish};
use rumqttc::v5::{AsyncClient, Event, EventLoop, MqttOptions};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use super::{
    Ack, Input, PubSub, Publisher, StatusEvent, StatusMessage, Task, WorkerInfo, s3_event,
    task_id_from_key,
};

/// Requests rumqttc buffers between the client handles and the event loop.
const REQUEST_CAPACITY: usize = 64;
/// How long to wait before reconnecting after the connection was lost.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct MqttSettings {
    /// `mqtt://host:port` (or `tcp://`); the port defaults to 1883.
    pub broker: String,
    /// Topic the bucket events arrive on, e.g. `dodoglass/ingest`.
    pub topic: String,
    /// Shared-subscription group: workers in the same group split the events.
    pub group: String,
    pub status_topic: String,
    /// Each worker's registration is published (retained) below this.
    pub workers_topic: String,
}

pub struct MqttPubSub {
    settings: MqttSettings,
    worker: String,
    client: AsyncClient,
    eventloop: Option<EventLoop>,
    tasks_tx: mpsc::UnboundedSender<Task>,
    tasks_rx: mpsc::UnboundedReceiver<Task>,
    /// Counts connections; an acknowledgement is only valid on the connection
    /// the message came in on (packet ids start over after a reconnect).
    connection: Arc<AtomicU64>,
    stopping: Arc<AtomicBool>,
    background: Option<JoinHandle<()>>,
}

#[derive(Clone)]
pub struct MqttPublisher {
    client: AsyncClient,
    topic: String,
}

impl MqttPubSub {
    /// `slots` is how many tasks this worker runs at once; the broker is told
    /// not to send more unfinished ones than that.
    pub fn new(settings: MqttSettings, worker: &str, slots: usize) -> Result<Self> {
        let (host, port) = parse_broker(&settings.broker)?;
        let mut options = MqttOptions::new(worker, host, port);
        options
            .set_keep_alive(Duration::from_secs(30))
            .set_clean_start(true)
            .set_manual_acks(true)
            .set_receive_maximum(Some(u16::try_from(slots).unwrap_or(u16::MAX).max(1)))
            // If the worker dies without unregistering, the broker clears its
            // registration by publishing this empty retained message.
            .set_last_will(LastWill::new(
                worker_topic(&settings, worker),
                Vec::new(),
                QoS::AtLeastOnce,
                true,
                None,
            ));
        let (client, eventloop) = AsyncClient::new(options, REQUEST_CAPACITY);
        let (tasks_tx, tasks_rx) = mpsc::unbounded_channel();
        Ok(MqttPubSub {
            settings,
            worker: worker.into(),
            client,
            eventloop: Some(eventloop),
            tasks_tx,
            tasks_rx,
            connection: Arc::new(AtomicU64::new(0)),
            stopping: Arc::new(AtomicBool::new(false)),
            background: None,
        })
    }
}

impl PubSub for MqttPubSub {
    type Publisher = MqttPublisher;

    async fn register(&mut self, worker: &WorkerInfo) -> Result<()> {
        let mut eventloop = self.eventloop.take().context("already registered")?;
        let broker = self.settings.broker.clone();

        // Wait for the first connection, so a wrong address fails right away
        // instead of retrying forever in the background.
        let connected = async {
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => return Ok(()),
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), connected)
            .await
            .map_err(|_| anyhow!("no answer from the MQTT broker {broker} within 10 s"))?
            .with_context(|| format!("cannot connect to the MQTT broker {broker}"))?;
        self.connection.store(1, Ordering::Relaxed);

        let background = Background {
            settings: self.settings.clone(),
            worker: self.worker.clone(),
            registration: serde_json::to_vec(worker)?,
            client: self.client.clone(),
            tasks: self.tasks_tx.clone(),
            connection: self.connection.clone(),
            stopping: self.stopping.clone(),
        };
        background.subscribe_and_announce();
        self.background = Some(tokio::spawn(background.run(eventloop)));
        info!(
            broker = %broker,
            subscription = %shared_topic(&self.settings),
            "connected to MQTT broker"
        );
        Ok(())
    }

    async fn next_task(&mut self) -> Result<Task> {
        self.tasks_rx
            .recv()
            .await
            .context("the MQTT connection was closed")
    }

    fn publisher(&self) -> MqttPublisher {
        MqttPublisher {
            client: self.client.clone(),
            topic: self.settings.status_topic.clone(),
        }
    }

    async fn unregister(&mut self) -> Result<()> {
        self.stopping.store(true, Ordering::Relaxed);
        // Same request queue as the acknowledgements of finished tasks, so
        // those are sent first.
        let topic = worker_topic(&self.settings, &self.worker);
        let _ = self
            .client
            .publish(topic, QoS::AtLeastOnce, true, Vec::new())
            .await;
        let _ = self.client.disconnect().await;
        if let Some(background) = self.background.take()
            && tokio::time::timeout(Duration::from_secs(5), background)
                .await
                .is_err()
        {
            warn!("MQTT connection did not close within 5 s");
        }
        Ok(())
    }
}

impl Publisher for MqttPublisher {
    async fn publish(&self, message: &StatusMessage) -> Result<()> {
        // try_publish never waits: if the broker is unreachable and the queue
        // is full, the status update is dropped instead of stalling the task
        // that reports it (and with it the ffmpeg process it is reading).
        self.client
            .try_publish(
                &self.topic,
                QoS::AtLeastOnce,
                false,
                serde_json::to_vec(message)?,
            )
            .context("status message not sent (MQTT queue full or connection closed)")
    }
}

/// Everything the background task needs.
struct Background {
    settings: MqttSettings,
    worker: String,
    registration: Vec<u8>,
    client: AsyncClient,
    tasks: mpsc::UnboundedSender<Task>,
    connection: Arc<AtomicU64>,
    stopping: Arc<AtomicBool>,
}

impl Background {
    async fn run(self, mut eventloop: EventLoop) {
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    // clean_start: a new session, so subscribe again.
                    self.connection.fetch_add(1, Ordering::Relaxed);
                    info!("reconnected to MQTT broker");
                    self.subscribe_and_announce();
                }
                Ok(Event::Incoming(Packet::Publish(publish))) => self.on_message(publish),
                Ok(Event::Incoming(Packet::SubAck(ack))) => {
                    debug!(reason_codes = ?ack.return_codes, "subscribed");
                }
                Ok(Event::Outgoing(Outgoing::Disconnect)) => break,
                Ok(_) => {}
                Err(_) if self.stopping.load(Ordering::Relaxed) => break,
                Err(e) => {
                    warn!(error = %e, "MQTT connection lost, reconnecting");
                    tokio::time::sleep(RECONNECT_DELAY).await;
                }
            }
        }
    }

    /// Non-blocking (`try_`) on purpose: this runs inside the loop that
    /// drains rumqttc's request queue, so waiting for space could deadlock.
    fn subscribe_and_announce(&self) {
        if let Err(e) = self
            .client
            .try_subscribe(shared_topic(&self.settings), QoS::AtLeastOnce)
        {
            warn!(error = %e, "could not subscribe");
        }
        let _ = self.client.try_publish(
            worker_topic(&self.settings, &self.worker),
            QoS::AtLeastOnce,
            true,
            self.registration.clone(),
        );
    }

    /// One MQTT message: an S3 event with one or more records.
    fn on_message(&self, publish: Publish) {
        let objects = match s3_event::objects(&publish.payload) {
            Ok(objects) => objects,
            Err(reason) => {
                warn!(%reason, "rejected MQTT message");
                self.reject("unknown", reason);
                let _ = self.client.try_ack(&publish);
                return;
            }
        };

        // The message is acknowledged when every task made from it is
        // finished, or right away if none is (the last clone is dropped).
        let ack = {
            let client = self.client.clone();
            let connection = self.connection.clone();
            let received_on = connection.load(Ordering::Relaxed);
            let publish = publish.clone();
            Ack::new(move || {
                if connection.load(Ordering::Relaxed) == received_on {
                    if let Err(e) = client.try_ack(&publish) {
                        warn!(error = %e, "could not acknowledge MQTT message");
                    }
                } else {
                    // The broker has dropped that connection's session and
                    // its packet ids; acknowledging now could hit another message.
                    debug!("not acknowledging: message came in on an earlier connection");
                }
            })
        };

        for object in objects {
            let task = Task {
                id: task_id_from_key(&object.key),
                input: Input::Object {
                    bucket: object.bucket,
                    key: object.key,
                    etag: object.etag,
                },
                formats: None,
                ack: ack.clone(),
            };
            if let Err(reason) = task.validate() {
                warn!(task = %task.id, %reason, "rejected task");
                self.reject(&task.id, reason);
                continue;
            }
            debug!(
                task = %task.id,
                input = %task.input,
                size_mb = format!("{:.1}", object.size as f64 / 1e6),
                "task received"
            );
            if self.tasks.send(task).is_err() {
                return; // the service is shutting down
            }
        }
    }

    fn reject(&self, task: &str, reason: String) {
        let mut status = StatusMessage::new(&self.worker, task, StatusEvent::Rejected);
        status.error = Some(reason);
        if let Ok(json) = serde_json::to_vec(&status) {
            let _ =
                self.client
                    .try_publish(&self.settings.status_topic, QoS::AtLeastOnce, false, json);
        }
    }
}

fn shared_topic(settings: &MqttSettings) -> String {
    format!("$share/{}/{}", settings.group, settings.topic)
}

fn worker_topic(settings: &MqttSettings, worker: &str) -> String {
    format!("{}/{worker}", settings.workers_topic)
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
