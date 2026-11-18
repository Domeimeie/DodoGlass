//! What the asset server talks to outside itself, behind interfaces that
//! tests replace with in-memory versions:
//!
//! - [`upload_events`]: the stream of upload events (MQTT in production);
//! - [`storage`]: the `ingest`, `final` and `assets` buckets (`object_store`);
//! - [`clock`]: the current time;
//! - [`tmdb`]: TMDB, where descriptive information comes from;
//! - [`status_hints`]: the transcoder's status messages (MQTT in production,
//!   [`mqtt_status`]); both MQTT connections share `mqtt_client`.

pub mod clock;
pub mod mqtt;
mod mqtt_client;
pub mod mqtt_status;
pub mod status_hints;
pub mod storage;
pub mod tmdb;
pub mod upload_events;
