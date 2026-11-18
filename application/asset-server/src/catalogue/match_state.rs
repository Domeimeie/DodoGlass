//! Where an entry's TMDB lookup stands: the `match_state` column of the
//! metadata tables, read and written as [`MatchState`].

use diesel::deserialize::{self, FromSql, FromSqlRow};
use diesel::expression::AsExpression;
use diesel::serialize::{self, IsNull, Output, ToSql};
use diesel::sql_types::Text;
use diesel::sqlite::{Sqlite, SqliteValue};

/// A Movie or Show is Matched, in Needs review or retrying; a Season or
/// Episode is Matched, not found (it keeps its number as its name) or
/// retrying. No metadata row = not looked up yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AsExpression, FromSqlRow)]
#[diesel(sql_type = Text)]
pub enum MatchState {
    Matched,
    NeedsReview,
    /// TMDB was unavailable: looked up again once `retry_at_ms` is due.
    Retrying,
    NotFound,
}

impl MatchState {
    fn as_str(self) -> &'static str {
        match self {
            MatchState::Matched => "matched",
            MatchState::NeedsReview => "needs_review",
            MatchState::Retrying => "retrying",
            MatchState::NotFound => "not_found",
        }
    }
}

impl ToSql<Text, Sqlite> for MatchState {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Sqlite>) -> serialize::Result {
        out.set_value(self.as_str());
        Ok(IsNull::No)
    }
}

impl FromSql<Text, Sqlite> for MatchState {
    fn from_sql(value: SqliteValue<'_, '_, '_>) -> deserialize::Result<Self> {
        let text = <String as FromSql<Text, Sqlite>>::from_sql(value)?;
        Ok(match text.as_str() {
            "matched" => MatchState::Matched,
            "needs_review" => MatchState::NeedsReview,
            "retrying" => MatchState::Retrying,
            "not_found" => MatchState::NotFound,
            other => return Err(format!("unknown match state {other:?}").into()),
        })
    }
}
