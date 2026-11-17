//! Ids shared by the transcoder and the asset server. Both derive the same
//! task id from an upload key, so the function lives here (ADR 0004).

use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

/// How many hex digits of the key's SHA-256 an id ends with.
const HASH_DIGITS: usize = 8;

/// The task id (also the output folder name in `final/`) for an object key:
/// a readable prefix, `-`, and the first 8 hex digits of SHA-256 over the
/// whole key in Unicode NFC.
///
/// `movies/The Matrix (1999)/matrix.mkv` →
/// `movies_The_Matrix__1999__matrix-<8 hex digits>`.
///
/// The prefix keeps `final/` browsable: the key without its extension, every
/// character other than ASCII letters, digits, `-`, `_` and `.` replaced by
/// `_`, leading dots trimmed, and `video` if nothing is left. The hash covers
/// the extension and the folders too, so keys that share a prefix
/// (`matrix.mkv` / `matrix.mp4`, `Amélie` / `Am lie`) still get different
/// ids, while the NFC and NFD spellings of a key get the same one.
pub fn task_id_from_key(key: &str) -> String {
    let key: String = key.nfc().collect();
    let hash = Sha256::digest(key.as_bytes());
    let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{}", readable_prefix(&key), &hex[..HASH_DIGITS])
}

/// The part of an id a person reads; not unique on its own.
fn readable_prefix(key: &str) -> String {
    let stem = match key.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => key,
    };
    let prefix: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let prefix = prefix.trim_start_matches('.');
    if prefix.is_empty() {
        "video".into()
    } else {
        prefix.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The readable part of an id, without the `-<hash>` suffix.
    fn prefix(id: &str) -> &str {
        id.rsplit_once('-').expect("an id has a hash suffix").0
    }

    #[test]
    fn readable_prefix_follows_the_old_rules() {
        let cases = [
            ("sample.mp4", "sample"),
            ("shows/Episode 1.mkv", "shows_Episode_1"),
            ("café.mov", "caf_"),
            (".hidden.mp4", "hidden"),
            (".mp4", "mp4"),
            ("...", "video"),
            (
                "movies/The Matrix (1999)/matrix.mkv",
                "movies_The_Matrix__1999__matrix",
            ),
        ];
        for (key, expected) in cases {
            assert_eq!(prefix(&task_id_from_key(key)), expected, "key {key:?}");
        }
    }

    /// Expected hashes from `printf '%s' "<key>" | sha256sum | cut -c1-8`.
    #[test]
    fn ids_end_in_the_hash_of_the_whole_key() {
        let cases = [
            ("sample.mp4", "sample-de8f39c7"),
            (
                "movies/The Matrix (1999)/matrix.mkv",
                "movies_The_Matrix__1999__matrix-48c3476c",
            ),
            (
                "movies/The Matrix (1999)/matrix.mp4",
                "movies_The_Matrix__1999__matrix-a8bb2d22",
            ),
            (
                "movies/Am\u{e9}lie (2001)/amelie.mkv",
                "movies_Am_lie__2001__amelie-7657ef7e",
            ),
            (
                "movies/Am lie (2001)/amelie.mkv",
                "movies_Am_lie__2001__amelie-a60c1094",
            ),
        ];
        for (key, expected) in cases {
            assert_eq!(task_id_from_key(key), expected, "key {key:?}");
        }
    }

    #[test]
    fn the_same_key_always_gives_the_same_id() {
        let key = "shows/Dark (2017)/Season 1/S01E01.mkv";
        assert_eq!(task_id_from_key(key), task_id_from_key(key));
    }

    #[test]
    fn nfc_and_nfd_spellings_give_the_same_id() {
        let nfc = "movies/Am\u{e9}lie (2001)/amelie.mkv";
        let nfd = "movies/Ame\u{301}lie (2001)/amelie.mkv";
        assert_ne!(nfc, nfd);
        assert_eq!(task_id_from_key(nfd), task_id_from_key(nfc));
        assert_eq!(
            task_id_from_key(nfd),
            "movies_Am_lie__2001__amelie-7657ef7e"
        );
    }
}
