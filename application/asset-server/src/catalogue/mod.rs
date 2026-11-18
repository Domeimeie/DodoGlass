//! The Catalogue: Movies and their Media files, and the rules about them.
//! Every change and every read of the catalogue goes through here; the HTTP
//! layer, the ingest listener and the reconciler stay thin.
//!
//! - [`placement`]: an upload seen in `ingest` becomes a Media file, placed
//!   from its path (ADR 0003);
//! - [`media_files`]: Media file states (Processing, Ready, Failed, Missing);
//! - [`movies`]: Movies, and what viewers see of them;
//! - [`shows`]: Shows, Seasons and Episodes, and what viewers see of them.

mod media_files;
mod movies;
mod placement;
pub(crate) mod schema;
mod shows;

use std::sync::Arc;

use crate::db::Db;
use crate::ports::clock::Clock;

pub use media_files::{
    Manifests, MediaFileState, MediaFileStatus, PROCESSING_TIMEOUT_MS, TIMED_OUT, TrackedMediaFile,
};
pub use movies::{MediaFileView, MovieView};
pub use shows::{EpisodeView, SeasonView, ShowView};

#[derive(Clone)]
pub struct Catalogue {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl Catalogue {
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Catalogue { db, clock }
    }
}
