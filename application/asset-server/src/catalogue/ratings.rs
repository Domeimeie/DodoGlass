//! Ratings: TMDB's user score and vote count of Movies, Shows and Episodes
//! (Seasons have none). Every Match and refresh sets the Rating with the
//! other fields; unlike them it is never a Locked field, and the rating
//! sweep (`crate::matcher`) keeps it current (ADR 0005). Each Rating records
//! when it was last fetched (`rating_updated_ms`).

use std::time::{Duration, UNIX_EPOCH};

use serde::Serialize;

use crate::ports::tmdb::Rating;

/// The field names of a Rating in the API: none of them can be edited or
/// locked.
pub const RATING_FIELDS: [&str; 3] = ["rating", "votes", "rating_updated_at"];

/// The Rating part of a Movie, Show or Episode in the read API: always
/// there, `null` without a Rating.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RatingView {
    /// TMDB's score, 0–10.
    pub rating: Option<f64>,
    pub votes: Option<i64>,
    /// When the Rating was last fetched (RFC 3339, UTC).
    pub rating_updated_at: Option<String>,
}

impl RatingView {
    /// From the `rating`, `votes` and `rating_updated_ms` columns.
    pub(super) fn from_columns(
        rating: Option<f64>,
        votes: Option<i64>,
        updated_ms: Option<i64>,
    ) -> RatingView {
        RatingView {
            rating,
            votes,
            rating_updated_at: updated_ms.map(timestamp),
        }
    }
}

/// Epoch milliseconds as RFC 3339 in UTC, to the second:
/// `2026-11-17T00:00:00Z`.
fn timestamp(ms: i64) -> String {
    let at = UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).unwrap_or(0));
    humantime::format_rfc3339_seconds(at).to_string()
}

/// The `rating` and `votes` columns of `rating`.
pub(super) fn columns(rating: Option<Rating>) -> (Option<f64>, Option<i64>) {
    (rating.map(|r| r.score), rating.map(|r| r.votes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_rfc3339_in_utc() {
        assert_eq!(timestamp(1_794_873_600_000), "2026-11-17T00:00:00Z");
        assert_eq!(timestamp(1_794_873_601_999), "2026-11-17T00:00:01Z");
    }
}
