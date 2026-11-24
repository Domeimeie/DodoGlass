//! The marker listener: each new output marker (`<id>/source.json`) in
//! `final` that RustFS publishes on `dodoglass/final` places its Media file
//! in the catalogue as Ready (ADR 0006), or makes it Ready again. The marker
//! is read from `final` as the reconcile reads it, so the two agree. Each
//! message is acknowledged once it is stored; delivery is at least once, and
//! handling the same marker twice changes nothing.

mod s3_event;

use tracing::{error, info, warn};

use crate::ports::marker_events::{MarkerEvents, MarkerMessage};
use crate::reconcile::{MarkerOutcome, Reconciler};

/// The file name of the output marker in each folder of `final`.
const MARKER: &str = "source.json";

/// Handles messages until the source closes.
pub async fn run(mut events: impl MarkerEvents, reconciler: Reconciler) {
    while let Some(message) = events.next().await {
        handle(&reconciler, message).await;
    }
}

async fn handle(reconciler: &Reconciler, message: MarkerMessage) {
    let objects = match s3_event::objects(&message.payload) {
        Ok(objects) => objects,
        Err(reason) => {
            // It would never parse, so there is no point in getting it again.
            warn!(%reason, "ignored message on the final topic");
            message.ack();
            return;
        }
    };
    for object in &objects {
        let Some(id) = media_file_id(reconciler.final_bucket(), &object.bucket, &object.key) else {
            warn!(bucket = %object.bucket, key = %object.key, "not an output marker: ignored");
            continue;
        };
        info!(media_file = %id, "output marker seen");
        match reconciler.marker_seen(id).await {
            Ok(MarkerOutcome::Stored) => {}
            Ok(MarkerOutcome::Unreadable(reason)) => {
                // It stays unreadable; the reconcile reports it again.
                warn!(media_file = %id, %reason, "unreadable output marker: ignored");
            }
            Err(e) => {
                // Not acknowledged: the broker delivers it again after a
                // reconnect, and placing is idempotent.
                error!(media_file = %id, error = format!("{e:#}"), "could not store output marker");
                return;
            }
        }
    }
    message.ack();
}

/// `final` + `<id>/source.json` → `<id>`; `None` for any other object.
fn media_file_id<'a>(final_bucket: &str, bucket: &str, key: &'a str) -> Option<&'a str> {
    let id = key.strip_suffix(MARKER)?.strip_suffix('/')?;
    (bucket == final_bucket && !id.is_empty() && !id.contains('/')).then_some(id)
}
