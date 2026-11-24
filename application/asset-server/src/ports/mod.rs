//! What the asset server talks to outside itself, behind interfaces that
//! tests replace with in-memory versions:
//!
//! - [`marker_events`]: the events of new output markers in `final` (MQTT in
//!   production, [`mqtt_markers`] on top of `mqtt_client`);
//! - [`storage`]: the `final` and `assets` buckets (`object_store`);
//! - [`clock`]: the current time;
//! - [`tmdb`]: TMDB, where descriptive information comes from.

pub mod clock;
pub mod marker_events;
mod mqtt_client;
pub mod mqtt_markers;
pub mod storage;
pub mod tmdb;
