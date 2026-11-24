//! Movie metadata from TMDB: an automatic Match, Needs review, retries while
//! TMDB is down, and posters.

mod common;

use std::time::Duration;

use asset_server::ports::tmdb::MovieDetails;
use axum::http::StatusCode;
use common::TestApp;
use common::tmdb::movie;
use serde_json::json;

const MATRIX: &str = "movies/The Matrix (1999)/matrix.mkv";

/// A Movie upload that is Ready (so listed), with its TMDB lookup done.
async fn ready_movie(app: &TestApp, key: &str) {
    app.finish_transcode(key).await;
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

#[tokio::test]
async fn the_poster_is_downloaded_and_served_from_images() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(MovieDetails {
        poster_path: Some("/f89U3ADr1oiB1s9GkdPOEpXUk5H.jpg".into()),
        ..movie(603, "The Matrix", 1999)
    });
    app.tmdb
        .add_image("/f89U3ADr1oiB1s9GkdPOEpXUk5H.jpg", b"the poster");

    ready_movie(&app, MATRIX).await;

    let image = app.get_json("/movies").await[0]["image"].clone();
    let key = image.as_str().expect("an image key");
    let (status, content_type, body) = app.get_bytes(&format!("/images/{key}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(body, b"the poster");

    app.tmdb.set_unreachable(true);
    let (status, _, body) = app.get_bytes(&format!("/images/{key}")).await;
    assert_eq!(
        (status, body.as_slice()),
        (StatusCode::OK, &b"the poster"[..])
    );
}

#[tokio::test]
async fn a_movie_without_a_poster_on_tmdb_has_no_image() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));

    ready_movie(&app, MATRIX).await;

    let listed = &app.get_json("/movies").await[0];
    assert_eq!(listed["tmdb_id"], json!(603));
    assert_eq!(listed.get("image"), None);
}

#[tokio::test]
async fn a_poster_tmdb_cannot_serve_still_matches_without_an_image() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(MovieDetails {
        poster_path: Some("/gone.jpg".into()),
        ..movie(603, "The Matrix", 1999)
    });

    ready_movie(&app, MATRIX).await;

    let listed = &app.get_json("/movies").await[0];
    assert_eq!(listed["tmdb_id"], json!(603));
    assert_eq!(listed.get("image"), None);
}

#[tokio::test]
async fn an_unknown_image_is_not_found() {
    let app = TestApp::start().await;

    let (status, _, _) = app.get_bytes("/images/posters/nothing.jpg").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn spider_man_and_spider_dash_man_are_the_same_name() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(557, "Spider-Man", 2002));

    ready_movie(&app, "movies/Spider Man (2002)/spider-man.mkv").await;

    let listed = &app.get_json("/movies").await[0];
    assert_eq!(listed["name"], json!("Spider-Man"));
    assert_eq!(listed["tmdb_id"], json!(557));
    assert_eq!(
        app.get_json("/admin/needs-review").await["movies"],
        json!([])
    );
}

/// A Movie in Needs review is listed under its folder's name and year, with
/// no metadata, and the admin API lists it with its candidates.
async fn assert_needs_review(app: &TestApp, candidates: serde_json::Value) {
    let listed = &app.get_json("/movies").await[0];
    assert_eq!(listed["name"], json!("The Matrix"));
    assert_eq!(listed["year"], json!(1999));
    assert_eq!(listed.get("tmdb_id"), None);
    assert_eq!(listed.get("description"), None);

    let review = app.get_json("/admin/needs-review").await;
    assert_eq!(review["movies"].as_array().unwrap().len(), 1, "{review}");
    assert_eq!(review["movies"][0]["name"], json!("The Matrix"));
    assert_eq!(review["movies"][0]["year"], json!(1999));
    assert_eq!(review["movies"][0]["candidates"], candidates);
}

#[tokio::test]
async fn no_result_needs_review_without_candidates() {
    let app = TestApp::start().await;

    ready_movie(&app, MATRIX).await;

    assert_needs_review(&app, json!([])).await;
}

#[tokio::test]
async fn a_result_from_another_year_needs_review() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1998));

    ready_movie(&app, MATRIX).await;

    assert_needs_review(
        &app,
        json!([{
            "tmdb_id": 603,
            "name": "The Matrix",
            "year": 1998,
            "description": "About The Matrix.",
        }]),
    )
    .await;
}

#[tokio::test]
async fn several_exact_results_need_review_with_the_top_five_kept() {
    let app = TestApp::start().await;
    for id in 1..=7 {
        app.tmdb.add_movie(movie(id, "The Matrix", 1999));
    }

    ready_movie(&app, MATRIX).await;

    let review = app.get_json("/admin/needs-review").await;
    let ids: Vec<_> = review["movies"][0]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["tmdb_id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [1, 2, 3, 4, 5]);
    assert_eq!(app.get_json("/movies").await[0].get("tmdb_id"), None);
}

#[tokio::test]
async fn while_tmdb_is_unreachable_the_lookup_is_retried_not_sent_to_review() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    app.tmdb.set_unreachable(true);

    ready_movie(&app, MATRIX).await;

    let listed = &app.get_json("/movies").await[0];
    assert_eq!(listed["name"], json!("The Matrix"));
    assert_eq!(listed.get("tmdb_id"), None);
    assert_eq!(
        app.get_json("/admin/needs-review").await["movies"],
        json!([])
    );

    app.tmdb.set_unreachable(false);
    app.match_metadata().await;
    assert_eq!(
        app.get_json("/movies").await[0].get("tmdb_id"),
        None,
        "the retry waits for its backoff"
    );

    app.advance(Duration::from_secs(30));
    app.match_metadata().await;
    assert_eq!(app.get_json("/movies").await[0]["tmdb_id"], json!(603));
}

#[tokio::test]
async fn a_long_outage_keeps_the_movie_retrying_hourly_and_out_of_review() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    app.tmdb.set_unreachable(true);

    ready_movie(&app, MATRIX).await;
    for _ in 0..(12 * 6) {
        app.advance(Duration::from_secs(5 * 60));
        app.match_metadata().await;
    }
    assert_eq!(
        app.get_json("/admin/needs-review").await["movies"],
        json!([]),
        "an outage does not fill the review queue"
    );

    app.tmdb.set_unreachable(false);
    app.advance(Duration::from_secs(3600));
    app.match_metadata().await;
    assert_eq!(app.get_json("/movies").await[0]["tmdb_id"], json!(603));
}

#[tokio::test]
async fn browsing_works_while_tmdb_is_unreachable() {
    let app = TestApp::start().await;
    app.tmdb.set_unreachable(true);

    ready_movie(&app, MATRIX).await;

    let listed = &app.get_json("/movies").await[0];
    assert_eq!(listed["media_files"][0]["label"], json!("matrix"));
}

#[tokio::test]
async fn a_second_media_file_of_a_matched_movie_does_not_call_tmdb_again() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    ready_movie(&app, MATRIX).await;
    let calls = app.tmdb.calls();

    ready_movie(&app, "movies/The Matrix (1999)/matrix-directors-cut.mkv").await;
    app.advance(Duration::from_secs(24 * 3600));
    app.match_metadata().await;

    assert_eq!(app.tmdb.calls(), calls);
    assert_eq!(app.get_json("/movies").await[0]["tmdb_id"], json!(603));
}
