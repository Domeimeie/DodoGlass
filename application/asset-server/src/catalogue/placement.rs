//! Placing a finished upload, from the input key its output marker records:
//! its path says which Movie or Episode it belongs to and gives it
//! its Label (ADR 0003); its key gives the Media file id (ADR 0004).

use anyhow::Result;
use diesel::Connection;
use dodoglass_ids::{Placement, place_upload, task_id_from_key};
use unicode_normalization::UnicodeNormalization;

use super::media_files::{Manifests, Owner, Placed};
use super::{Catalogue, media_files, movies, shows};

impl Catalogue {
    /// An output marker in `final` for a Media file the catalogue does not
    /// know: the Media file for input `key` is placed in the Movie or Episode
    /// its path names (an Unassigned media file if the path doesn't fit), and
    /// is Ready with `manifests`. Placing the same key again is idempotent.
    pub async fn output_seen(&self, key: &str, manifests: Manifests) -> Result<()> {
        // The raw key is what `ingest` holds (S3 keys are byte-exact), so it is
        // stored for every bucket call and shown as is. Only placing reads its
        // NFC form, so that a name typed on a Mac and on Linux is the same Movie
        // or Show; the id is NFC-based already (ADR 0004).
        let key = key.to_string();
        let nfc: String = key.nfc().collect();
        let id = task_id_from_key(&key);
        let now = self.clock.now_ms();
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let (owner, stem) = match place_upload(&nfc) {
                        Placement::Movie { name, year, label } => {
                            let movie = movies::find_or_create(conn, &name, year)?;
                            (Some(Owner::Movie(movie)), label)
                        }
                        Placement::Episode {
                            show,
                            year,
                            season,
                            episode: number,
                            label,
                        } => {
                            let place = shows::EpisodePlace {
                                show: &show,
                                year,
                                season,
                                episode: number,
                            };
                            let episode = shows::find_or_create(conn, place)?;
                            (Some(Owner::Episode(episode)), label)
                        }
                        Placement::Unassigned(_) => (None, file_stem(&nfc).to_string()),
                    };
                    let placed = Placed {
                        id: &id,
                        key: &key,
                        owner,
                        stem: &stem,
                    };
                    media_files::place_ready(conn, placed, manifests, now)
                })
            })
            .await
    }
}

/// `misc/clip.mp4` → `clip`.
fn file_stem(key: &str) -> &str {
    let name = key.rsplit('/').next().unwrap_or(key);
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}
