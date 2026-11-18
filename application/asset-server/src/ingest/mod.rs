//! The ingest listener: turns upload events into "upload seen" calls on the
//! Catalogue, and acknowledges each message once they are stored.

mod s3_event;

use tracing::{debug, error, warn};

use crate::catalogue::Catalogue;
use crate::ports::upload_events::{UploadEvents, UploadMessage};

/// Handles messages until the source closes.
pub async fn run(mut events: impl UploadEvents, catalogue: Catalogue) {
    while let Some(message) = events.next().await {
        handle(&catalogue, message).await;
    }
}

async fn handle(catalogue: &Catalogue, message: UploadMessage) {
    let objects = match s3_event::objects(&message.payload) {
        Ok(objects) => objects,
        Err(reason) => {
            // It would never parse, so there is no point in getting it again.
            warn!(%reason, "ignored message on the ingest topic");
            message.ack();
            return;
        }
    };
    for object in &objects {
        debug!(bucket = %object.bucket, key = %object.key, "upload seen");
        if let Err(e) = catalogue.upload_seen(&object.key).await {
            // Not acknowledged: the broker delivers it again after a
            // reconnect, and placing is idempotent.
            error!(key = %object.key, error = format!("{e:#}"), "could not store upload");
            return;
        }
    }
    message.ack();
}
