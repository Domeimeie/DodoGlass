//! The status listener: the transcoder's `completed` and `failed` messages
//! on `dodoglass/status` are hints. `completed` triggers an early reconcile
//! of that Media file (the output marker still decides Ready, ADR 0001);
//! `failed` makes a Processing Media file Failed with the error. Every other
//! event is ignored.

use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::catalogue::Catalogue;
use crate::ports::status_hints::StatusHints;
use crate::reconcile::Reconciler;

/// The parts of a status message the asset server reads.
#[derive(Deserialize)]
struct StatusMessage {
    /// The task id, which is the Media file id.
    task: String,
    event: String,
    #[serde(default)]
    error: Option<String>,
}

/// Handles messages until the source closes.
pub async fn run(mut hints: impl StatusHints, catalogue: Catalogue, reconciler: Reconciler) {
    while let Some(hint) = hints.next().await {
        handle(&catalogue, &reconciler, &hint.payload).await;
        hint.handled();
    }
}

async fn handle(catalogue: &Catalogue, reconciler: &Reconciler, payload: &[u8]) {
    let message: StatusMessage = match serde_json::from_slice(payload) {
        Ok(message) => message,
        Err(e) => {
            debug!(error = %e, "ignored message on the status topic");
            return;
        }
    };
    let id = &message.task;
    let result = match message.event.as_str() {
        "completed" => reconciler.run_for(id).await,
        "failed" => {
            let error = message.error.as_deref().unwrap_or("transcode failed");
            info!(media_file = %id, %error, "failed status");
            catalogue.transcode_failed(id, error).await
        }
        _ => Ok(()),
    };
    // A hint that could not be handled is caught by the next reconcile.
    if let Err(e) = result {
        warn!(media_file = %id, error = format!("{e:#}"), "could not handle status hint");
    }
}
