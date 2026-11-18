//! Admin metadata corrections: picking a Match, editing (and so locking)
//! fields, unlocking them, refreshing from TMDB, and attaching an Unassigned
//! media file to a new Movie or Show created from a TMDB id.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use common::tmdb::movie;
use serde_json::{Value, json};

const MATRIX: &str = "movies/The Matrix (1999)/matrix.mkv";

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

/// TMDB knows two Movies called The Matrix from 1999: Needs review.
fn two_matrices(app: &TestApp) {
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    app.tmdb.add_movie(movie(9999, "The Matrix", 1999));
}

#[tokio::test]
async fn picking_a_candidate_matches_the_movie_and_takes_it_off_needs_review() {
    let app = TestApp::start().await;
    two_matrices(&app);
    ready(&app, MATRIX).await;
    let review = app.get_json("/admin/needs-review").await;
    let entry = &review["movies"][0];
    assert_eq!(entry["candidates"].as_array().unwrap().len(), 2);
    let id = &entry["id"];
    let picked = &entry["candidates"][0]["tmdb_id"];

    let (status, body) = app
        .send_json(
            "PUT",
            &format!("/admin/movies/{id}/match"),
            json!({ "tmdb_id": picked }),
        )
        .await;

    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let listed = only_movie(&app).await;
    assert_eq!(&listed["tmdb_id"], picked);
    assert_eq!(listed["description"], json!("About The Matrix."));
    let review = app.get_json("/admin/needs-review").await;
    assert_eq!(review["movies"], json!([]));
}

#[tokio::test]
async fn any_tmdb_id_can_be_entered_as_the_match() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    ready(&app, "movies/Matrix (1999)/matrix.mkv").await;
    let id = &app.get_json("/admin/needs-review").await["movies"][0]["id"];

    let (status, body) = app
        .send_json(
            "PUT",
            &format!("/admin/movies/{id}/match"),
            json!({ "tmdb_id": 603 }),
        )
        .await;

    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let listed = only_movie(&app).await;
    assert_eq!(listed["name"], json!("The Matrix"));
    assert_eq!(listed["tmdb_id"], json!(603));
}

#[tokio::test]
async fn a_match_tmdb_does_not_know_or_cannot_answer_is_refused() {
    let app = TestApp::start().await;
    ready(&app, MATRIX).await;
    let id = &app.get_json("/admin/needs-review").await["movies"][0]["id"];
    let uri = format!("/admin/movies/{id}/match");

    let (status, _) = app.send_json("PUT", &uri, json!({ "tmdb_id": 404 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    app.tmdb.set_unreachable(true);
    let (status, _) = app.send_json("PUT", &uri, json!({ "tmdb_id": 603 })).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let (status, _) = app
        .send_json("PUT", "/admin/movies/999/match", json!({ "tmdb_id": 603 }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let review = app.get_json("/admin/needs-review").await;
    assert_eq!(review["movies"][0]["id"], *id);
}
