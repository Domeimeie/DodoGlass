//! What the asset server talks to outside itself, behind interfaces that
//! tests replace with in-memory versions:
//!
//! - [`upload_events`]: the stream of upload events (MQTT in production);
//! - [`storage`]: the `ingest`, `final` and `assets` buckets (`object_store`);
//! - [`clock`]: the current time.

pub mod clock;
pub mod mqtt;
pub mod storage;
pub mod upload_events;
