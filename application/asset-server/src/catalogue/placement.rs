//! Placing an upload: its path says which Movie it belongs to and gives it
//! its Label (ADR 0003); its key gives the Media file id (ADR 0004).

use anyhow::Result;
use diesel::Connection;
use dodoglass_ids::{Placement, place_upload, task_id_from_key};
use unicode_normalization::UnicodeNormalization;

use super::{Catalogue, media_files, movies};

impl Catalogue {
    /// An upload to `ingest` was seen: the Media file for `key` is now
    /// Processing, in the Movie its path names. Seeing the same key again is
    /// a new version of the same Media file.
    pub async fn upload_seen(&self, key: &str) -> Result<()> {
        // NFC, so that a name typed on a Mac and on Linux is the same path.
        let key: String = key.nfc().collect();
        let id = task_id_from_key(&key);
        let now = self.clock.now_ms();
        self.db
            .run(move |conn| {
                conn.transaction(|conn| {
                    let (movie, label) = match place_upload(&key) {
                        Placement::Movie { name, year, label } => {
                            (Some(movies::find_or_create(conn, &name, year)?), label)
                        }
                        // Shows come in a later ticket; until then an Episode
                        // upload waits like an Unassigned one.
                        Placement::Episode { .. } | Placement::Unassigned(_) => {
                            (None, file_stem(&key).to_string())
                        }
                    };
                    media_files::processing(conn, &id, &key, movie, &label, now)
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
