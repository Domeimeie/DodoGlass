//! DodoGlass asset server: the catalogue of Movies and Shows (Seasons,
//! Episodes) and their Media files.
//!
//! - [`catalogue`]: the core, which owns the catalogue and its rules;
//! - [`ports`]: marker events, object storage, the clock and TMDB, replaceable in tests;
//! - [`markers`]: the marker listener (new output markers in `final` → Ready);
//! - [`reconcile`]: the reconciler (output markers in `final` → Ready);
//! - [`matcher`]: TMDB lookups for new Movies and Shows (Match, Needs review,
//!   retries), and Seasons and Episodes of matched Shows;
//! - [`http`]: the HTTP API;
//! - [`app`]: puts them together, for `main` and for the tests.

pub mod app;
pub mod catalogue;
pub mod db;
pub mod http;
pub mod markers;
pub mod matcher;
pub mod ports;
pub mod reconcile;
