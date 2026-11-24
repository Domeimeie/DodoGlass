//! Ratings: TMDB's user score and vote count of Movies, Shows and Episodes,
//! set with every Match and refresh, never locked, and kept current by the
//! rating sweep (ADR 0005).

mod common;

use asset_server::ports::tmdb::MovieDetails;
use common::TestApp;
use common::tmdb::{movie, rated};
use serde_json::{Value, json};

const MATRIX: &str = "movies/The Matrix (1999)/matrix.mkv";

/// The fake clock's start, as the API writes timestamps.
const START: &str = "2026-11-17T00:00:00Z";

/// Uploads `key`, lets its transcode finish and runs a TMDB pass.
async fn ready(app: &TestApp, key: &str) {
    app.upload(key).await;
    app.finish_transcode(key).await;
    app.reconcile().await;
    app.match_metadata().await;
}

/// The only listed Movie.
async fn only_movie(app: &TestApp) -> Value {
    let movies = app.get_json("/movies").await;
    assert_eq!(movies.as_array().unwrap().len(), 1, "{movies}");
    movies[0].clone()
}

#[tokio::test]
async fn a_matched_movie_shows_tmdbs_score_and_vote_count() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(MovieDetails {
        rating: rated(8.2, 26_000),
        ..movie(603, "The Matrix", 1999)
    });

    ready(&app, MATRIX).await;

    let listed = only_movie(&app).await;
    assert_eq!(listed["rating"], json!(8.2));
    assert_eq!(listed["votes"], json!(26_000));
    assert_eq!(listed["rating_updated_at"], json!(START));
}

#[tokio::test]
async fn an_unmatched_movie_has_no_rating() {
    let app = TestApp::start().await;

    ready(&app, MATRIX).await;

    let listed = only_movie(&app).await;
    assert_eq!(listed["rating"], Value::Null);
    assert_eq!(listed["votes"], Value::Null);
    assert_eq!(listed["rating_updated_at"], Value::Null);
    let fields = listed.as_object().unwrap();
    assert!(fields.contains_key("rating") && fields.contains_key("rating_updated_at"));
}
