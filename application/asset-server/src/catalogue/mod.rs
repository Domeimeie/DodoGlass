//! The Catalogue: Movies and Shows, their Media files, and the rules about them.
//! Every change and every read of the catalogue goes through here; the HTTP
//! layer, the marker listener and the reconciler stay thin.
//!
//! - [`placement`]: a finished upload becomes a Media file, placed from its
//!   path (ADR 0003);
//! - [`media_files`]: Media file states (Ready, Missing);
//! - [`match_state`]: where a TMDB lookup stands (Matched, Needs review, ...);
//! - [`movies`]: Movies, and what viewers see of them;
//! - [`shows`]: Shows, Seasons and Episodes, and what viewers see of them;
//! - [`unassigned`]: Unassigned media files, and attaching them;
//! - [`metadata`]: what TMDB lookups found (Match, Needs review, retries)
//!   and Locked fields of Movies;
//! - [`show_metadata`]: the same for Shows, and their Seasons and Episodes
//!   looked up by number;
//! - [`ratings`]: Ratings (TMDB's user score), never locked and kept
//!   current by the rating sweep (ADR 0005);
//! - [`corrections`]: the admin's metadata corrections: fields edited by
//!   hand (Locked fields), and the entries a picked Match or a refresh
//!   applies to;
//! - [`admin`]: what an admin changes: the Default media file, Labels,
//!   removing Media files and deleting Hidden entries.

mod admin;
mod corrections;
mod match_state;
mod media_files;
mod metadata;
mod movies;
mod placement;
mod ratings;
pub(crate) mod schema;
mod show_metadata;
mod shows;
mod unassigned;

use std::sync::Arc;

use object_store::ObjectStore;

use crate::db::Db;
use crate::ports::clock::Clock;

pub use admin::{Change, Entry};
pub use corrections::{Edited, MovieLookup, PartRefresh, ShowLookup};
pub use media_files::{Manifests, MediaFileState, MediaFileStatus, TrackedMediaFile};
pub use metadata::{
    Candidate, LOCKABLE_FIELDS, LockedFields, LookupDue, MovieMatch, MovieMetadataView,
    NeedsReviewMovie,
};
pub use movies::{MediaFileView, MovieView};
pub use ratings::{RATING_FIELDS, RatingView, StaleRating};
pub use show_metadata::{
    EPISODE_LOCKABLE_FIELDS, EpisodeMatch, NeedsReviewShow, Part, PartLookupDue,
    SEASON_LOCKABLE_FIELDS, SHOW_LOCKABLE_FIELDS, SeasonMatch, ShowMatch, ShowMetadataView,
};
pub use shows::{EpisodeView, SeasonView, ShowView};
pub use unassigned::{AttachTo, Attached, UnassignedView};

#[derive(Clone)]
pub struct Catalogue {
    db: Db,
    clock: Arc<dyn Clock>,
    /// The `final` bucket, where removing a Media file deletes its output.
    final_: Arc<dyn ObjectStore>,
}

impl Catalogue {
    pub fn new(db: Db, clock: Arc<dyn Clock>, final_: Arc<dyn ObjectStore>) -> Self {
        Catalogue { db, clock, final_ }
    }
}
