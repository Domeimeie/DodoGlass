//! The Catalogue: Movies and their Media files, and the rules about them.
//! Every change and every read of the catalogue goes through here; the HTTP
//! layer, the ingest listener and the reconciler stay thin.
//!
//! - [`placement`]: an upload seen in `ingest` becomes a Media file, placed
//!   from its path (ADR 0003);
//! - [`media_files`]: Media file states (Processing, Ready, ...);
//! - [`movies`]: Movies, and what viewers see of them;
//! - [`metadata`]: what TMDB lookups found (Match, Needs review, retries)
//!   and Locked fields.

mod media_files;
mod metadata;
mod movies;
mod placement;
pub(crate) mod schema;

use std::sync::Arc;

use crate::db::Db;
use crate::ports::clock::Clock;

pub use media_files::Manifests;
pub use metadata::{
    Candidate, LOCKABLE_FIELDS, LockedFields, MovieMatch, MovieMetadataView, NeedsReviewMovie,
    PendingLookup,
};
pub use movies::{MediaFileView, MovieView};

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
