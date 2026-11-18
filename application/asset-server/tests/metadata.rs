//! Movie metadata from TMDB: an automatic Match, Needs review, retries while
//! TMDB is down, and posters.

mod common;

use common::TestApp;
use common::tmdb::movie;
use serde_json::json;

const MATRIX: &str = "movies/The Matrix (1999)/matrix.mkv";

/// A Movie upload that is Ready (so listed), with its TMDB lookup done.
async fn ready_movie(app: &TestApp, key: &str) {
    app.upload(key).await;
    app.finish_transcode(key).await;
    app.reconcile().await;
    app.match_metadata().await;
}

#[tokio::test]
async fn one_exact_result_is_matched_and_fills_the_movie() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    app.tmdb.add_movie(movie(604, "The Matrix Reloaded", 2003));

    ready_movie(&app, MATRIX).await;

    let listed = &app.get_json("/movies").await[0];
    assert_eq!(listed["name"], json!("The Matrix"));
    assert_eq!(listed["year"], json!(1999));
    assert_eq!(listed["tmdb_id"], json!(603));
    assert_eq!(listed["imdb_id"], json!("tt0000603"));
    assert_eq!(listed["description"], json!("About The Matrix."));
}
