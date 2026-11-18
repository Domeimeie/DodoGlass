//! Admin metadata corrections: picking a Match, editing (and so locking)
//! fields, unlocking them, refreshing from TMDB, and attaching an Unassigned
//! media file to a new Movie or Show created from a TMDB id.

mod common;

use asset_server::ports::tmdb::MovieDetails;
use axum::body::Body;
use axum::http::{Request, StatusCode};
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

#[tokio::test]
async fn an_edited_field_is_locked_and_kept_when_another_match_is_picked() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    app.tmdb.add_movie(movie(9999, "The Matrix Remade", 1999));
    ready(&app, MATRIX).await;
    let id = only_movie(&app).await["id"].clone();

    let (status, body) = app
        .send_json(
            "PATCH",
            &format!("/admin/movies/{id}"),
            json!({ "description": "My own words." }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["locked_fields"], json!(["description"]));
    assert_eq!(
        only_movie(&app).await["description"],
        json!("My own words.")
    );

    let (status, _) = app
        .send_json(
            "PUT",
            &format!("/admin/movies/{id}/match"),
            json!({ "tmdb_id": 9999 }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let listed = only_movie(&app).await;
    assert_eq!(listed["description"], json!("My own words."));
    assert_eq!(listed["name"], json!("The Matrix Remade"));
    assert_eq!(listed["imdb_id"], json!("tt0009999"));
}

#[tokio::test]
async fn a_refresh_pulls_in_tmdb_changes_except_locked_fields_until_unlocked() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    ready(&app, MATRIX).await;
    let id = only_movie(&app).await["id"].clone();
    app.send_json(
        "PATCH",
        &format!("/admin/movies/{id}"),
        json!({ "name": "Matrix", "year": 2000 }),
    )
    .await;
    app.tmdb.add_movie(MovieDetails {
        overview: Some("Neo wakes up.".into()),
        ..movie(603, "The Matrix", 1999)
    });

    let (status, body) = app
        .send_json("POST", &format!("/admin/movies/{id}/refresh"), json!({}))
        .await;

    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let listed = only_movie(&app).await;
    assert_eq!(listed["description"], json!("Neo wakes up."));
    assert_eq!(listed["name"], json!("Matrix"));
    assert_eq!(listed["year"], json!(2000));

    let (status, body) = app
        .request(
            Request::delete(format!("/admin/movies/{id}/locks/name"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["locked_fields"], json!(["year"]));
    app.send_json("POST", &format!("/admin/movies/{id}/refresh"), json!({}))
        .await;

    let listed = only_movie(&app).await;
    assert_eq!(listed["name"], json!("The Matrix"));
    assert_eq!(listed["year"], json!(2000));
}

#[tokio::test]
async fn a_movie_edited_before_its_first_lookup_is_still_looked_up() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    app.upload(MATRIX).await;
    app.finish_transcode(MATRIX).await;
    app.reconcile().await;
    let id = only_movie(&app).await["id"].clone();
    app.send_json(
        "PATCH",
        &format!("/admin/movies/{id}"),
        json!({ "description": "My own words." }),
    )
    .await;

    app.match_metadata().await;

    let listed = only_movie(&app).await;
    assert_eq!(listed["tmdb_id"], json!(603));
    assert_eq!(listed["description"], json!("My own words."));
}

#[tokio::test]
async fn a_movie_in_needs_review_shows_its_edited_fields() {
    let app = TestApp::start().await;
    two_matrices(&app);
    ready(&app, MATRIX).await;
    let id = only_movie(&app).await["id"].clone();

    app.send_json(
        "PATCH",
        &format!("/admin/movies/{id}"),
        json!({ "description": "My own words.", "imdb_id": "tt0133093" }),
    )
    .await;

    let listed = only_movie(&app).await;
    assert_eq!(listed["description"], json!("My own words."));
    assert_eq!(listed["imdb_id"], json!("tt0133093"));
    let review = app.get_json("/admin/needs-review").await;
    assert_eq!(review["movies"][0]["id"], id);
}

#[tokio::test]
async fn an_edit_of_a_field_the_entry_does_not_have_is_refused() {
    let app = TestApp::start().await;
    ready(&app, MATRIX).await;
    let id = only_movie(&app).await["id"].clone();
    let uri = format!("/admin/movies/{id}");

    for bad in [
        json!({ "air_date": "1999-03-31" }),
        json!({ "year": "1999" }),
        json!({ "name": "" }),
        json!({}),
    ] {
        let (status, _) = app.send_json("PATCH", &uri, bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (status, _) = app
        .send_json("PATCH", "/admin/movies/999", json!({ "name": "X" }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(only_movie(&app).await["name"], json!("The Matrix"));
}

#[tokio::test]
async fn a_re_upload_keeps_locked_fields() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    ready(&app, MATRIX).await;
    let id = only_movie(&app).await["id"].clone();
    app.send_json(
        "PATCH",
        &format!("/admin/movies/{id}"),
        json!({ "name": "Matrix", "description": "My own words." }),
    )
    .await;

    ready(&app, MATRIX).await;

    let listed = only_movie(&app).await;
    assert_eq!(listed["name"], json!("Matrix"));
    assert_eq!(listed["description"], json!("My own words."));
    assert_eq!(listed["tmdb_id"], json!(603));
}

#[tokio::test]
async fn a_movie_whose_last_media_file_was_removed_comes_back_with_its_locked_fields() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    ready(&app, MATRIX).await;
    let listed = only_movie(&app).await;
    let id = &listed["id"];
    app.send_json(
        "PATCH",
        &format!("/admin/movies/{id}"),
        json!({ "name": "Matrix", "description": "My own words." }),
    )
    .await;
    let file = listed["media_files"][0]["id"].as_str().unwrap();
    let (status, _) = app
        .request(
            Request::delete(format!("/admin/media-files/{file}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(app.get_json("/movies").await, json!([]));

    ready(&app, MATRIX).await;
    app.send_json("POST", &format!("/admin/movies/{id}/refresh"), json!({}))
        .await;

    let listed = only_movie(&app).await;
    assert_eq!(&listed["id"], id);
    assert_eq!(listed["name"], json!("Matrix"));
    assert_eq!(listed["description"], json!("My own words."));
    assert_eq!(listed["tmdb_id"], json!(603));
}
