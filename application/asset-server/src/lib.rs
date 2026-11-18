//! DodoGlass asset server: the catalogue of Movies and Shows (Seasons,
//! Episodes) and their Media files.
//!
//! - [`catalogue`]: the core, which owns the catalogue and its rules;
//! - [`ports`]: upload events, object storage and the clock, replaceable in tests;
//! - [`ingest`]: the ingest listener (upload events → Catalogue);
//! - [`reconcile`]: the reconciler (output markers in `final` → Ready);
//! - [`status`]: the status listener (status hints → early reconcile, Failed);
//! - [`matcher`]: TMDB lookups for new Movies and Shows (Match, Needs review,
//!   retries), and Seasons and Episodes of matched Shows;
//! - [`http`]: the HTTP API;
//! - [`app`]: puts them together, for `main` and for the tests.

pub mod app;
pub mod catalogue;
pub mod db;
pub mod http;
pub mod ingest;
pub mod matcher;
pub mod ports;
pub mod reconcile;
pub mod status;
