//! Admin management of Media files: setting the Default media file, renaming
//! Labels, removing Media files, deleting Hidden entries, and listing Failed
//! and Missing Media files.

mod common;

use std::time::Duration;

use common::TestApp;
use common::tmdb::movie;
use futures_util::StreamExt;
use object_store::ObjectStore;
use serde_json::{Value, json};

const MATRIX: &str = "movies/The Matrix (1999)/matrix.mkv";
const DIRECTORS_CUT: &str = "movies/The Matrix (1999)/matrix-directors-cut.mkv";
const EXTENDED: &str = "movies/The Matrix (1999)/matrix-extended.mkv";
const PILOT: &str = "shows/Breaking Bad (2008)/Season 01/S01E01.mkv";
const PILOT_HD: &str = "shows/Breaking Bad (2008)/Season 01/S01E01 hd.mkv";

const MINUTE: Duration = Duration::from_secs(60);

/// The output of `key` disappears from `final`.
async fn lose_output(app: &TestApp, key: &str) {
    app.delete_prefix(&app.buckets.final_, &format!("{}/", id(key)))
        .await;
}

fn id(key: &str) -> String {
    dodoglass_ids::task_id_from_key(key)
}

fn uri(key: &str) -> String {
    format!("/admin/media-files/{}", id(key))
}

async fn upload_ready(app: &TestApp, key: &str) {
    app.upload(key).await;
    app.finish_transcode(key).await;
    app.reconcile().await;
}

/// The Label of the default Media file of the first listed Movie, if any.
async fn movie_default(app: &TestApp) -> Option<String> {
    default_of(&app.get_json("/movies").await[0])
}

fn default_of(entry: &Value) -> Option<String> {
    entry["media_files"]
        .as_array()?
        .iter()
        .find(|f| f["default"] == json!(true))
        .map(|f| f["label"].as_str().unwrap().to_string())
}

async fn labels(app: &TestApp) -> Vec<String> {
    app.get_json("/movies").await[0]["media_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["label"].as_str().unwrap().to_string())
        .collect()
}

async fn put(app: &TestApp, uri: &str) -> axum::http::StatusCode {
    app.send_json("PUT", uri, json!({})).await.0
}

async fn delete(app: &TestApp, uri: &str) -> axum::http::StatusCode {
    app.send_json("DELETE", uri, Value::Null).await.0
}

// --- the Default media file ---------------------------------------------------

#[tokio::test]
async fn setting_the_default_changes_which_media_file_the_read_api_marks_as_default() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    upload_ready(&app, DIRECTORS_CUT).await;
    assert_eq!(movie_default(&app).await.as_deref(), Some("matrix"));

    let status = put(&app, &format!("{}/default", uri(DIRECTORS_CUT))).await;

    assert_eq!(status, 204);
    assert_eq!(
        movie_default(&app).await.as_deref(),
        Some("matrix-directors-cut")
    );
}

#[tokio::test]
async fn a_media_file_that_is_not_ready_cannot_be_made_the_default() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    app.upload(DIRECTORS_CUT).await;

    let status = put(&app, &format!("{}/default", uri(DIRECTORS_CUT))).await;

    assert_eq!(status, 409);
    assert_eq!(movie_default(&app).await.as_deref(), Some("matrix"));
}

#[tokio::test]
async fn an_unknown_media_file_cannot_be_made_the_default() {
    let app = TestApp::start().await;

    let status = put(&app, "/admin/media-files/nope-00000000/default").await;

    assert_eq!(status, 404);
}

#[tokio::test]
async fn when_the_default_goes_missing_the_oldest_remaining_ready_one_takes_over() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    app.advance(MINUTE);
    upload_ready(&app, DIRECTORS_CUT).await;
    app.advance(MINUTE);
    upload_ready(&app, EXTENDED).await;
    put(&app, &format!("{}/default", uri(EXTENDED))).await;

    lose_output(&app, EXTENDED).await;
    app.reconcile().await;

    assert_eq!(movie_default(&app).await.as_deref(), Some("matrix"));
}

#[tokio::test]
async fn when_the_default_fails_the_oldest_remaining_ready_one_takes_over() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    app.advance(MINUTE);
    upload_ready(&app, DIRECTORS_CUT).await;

    // A new version of the default, whose transcode fails.
    lose_output(&app, MATRIX).await;
    app.upload(MATRIX).await;
    app.fail_transcode(MATRIX, "ffmpeg exited with 1").await;

    assert_eq!(
        movie_default(&app).await.as_deref(),
        Some("matrix-directors-cut")
    );
}

#[tokio::test]
async fn a_missing_default_with_no_other_ready_media_file_stays_the_default() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    app.upload(DIRECTORS_CUT).await;

    lose_output(&app, MATRIX).await;
    app.reconcile().await;
    app.finish_transcode(MATRIX).await;
    app.reconcile().await;

    assert_eq!(movie_default(&app).await.as_deref(), Some("matrix"));
}

#[tokio::test]
async fn the_default_of_an_episode_hands_over_too() {
    let app = TestApp::start().await;
    upload_ready(&app, PILOT).await;
    app.advance(MINUTE);
    upload_ready(&app, PILOT_HD).await;

    lose_output(&app, PILOT).await;
    app.reconcile().await;

    let show = app.get_json("/shows").await[0]["id"].clone();
    let episodes = app
        .get_json(&format!("/shows/{show}/seasons/1/episodes"))
        .await;
    assert_eq!(default_of(&episodes[0]).as_deref(), Some("S01E01 hd"));
}

// --- Labels -----------------------------------------------------------------

async fn rename(app: &TestApp, key: &str, label: &str) -> axum::http::StatusCode {
    app.send_json("PATCH", &uri(key), json!({ "label": label }))
        .await
        .0
}

#[tokio::test]
async fn an_admin_renames_a_label() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    upload_ready(&app, DIRECTORS_CUT).await;

    let status = rename(&app, DIRECTORS_CUT, "Director's Cut").await;

    assert_eq!(status, 204);
    assert_eq!(labels(&app).await, vec!["Director's Cut", "matrix"]);
}

#[tokio::test]
async fn renaming_to_a_label_taken_in_the_same_movie_is_refused() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    upload_ready(&app, DIRECTORS_CUT).await;

    let status = rename(&app, DIRECTORS_CUT, "matrix").await;

    assert_eq!(status, 409);
    assert_eq!(labels(&app).await, vec!["matrix", "matrix-directors-cut"]);
}

#[tokio::test]
async fn a_label_taken_in_another_movie_is_free() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    upload_ready(&app, "movies/Heat (1995)/heat.mkv").await;

    let status = rename(&app, MATRIX, "heat").await;

    assert_eq!(status, 204);
}

#[tokio::test]
async fn an_empty_label_is_refused() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;

    assert_eq!(rename(&app, MATRIX, "  ").await, 400);
    assert_eq!(labels(&app).await, vec!["matrix"]);
}

#[tokio::test]
async fn renaming_an_unknown_media_file_is_not_found() {
    let app = TestApp::start().await;

    assert_eq!(rename(&app, MATRIX, "matrix").await, 404);
}

// --- removing a Media file ----------------------------------------------------

/// The keys under `<id of key>/` in `final`.
async fn outputs(app: &TestApp, key: &str) -> Vec<String> {
    let prefix = object_store::path::Path::parse(id(key)).unwrap();
    let mut keys: Vec<String> = app
        .buckets
        .final_
        .list(Some(&prefix))
        .map(|o| o.unwrap().location.to_string())
        .collect()
        .await;
    keys.sort();
    keys
}

#[tokio::test]
async fn removing_a_media_file_deletes_its_output_in_final_and_its_entry() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    upload_ready(&app, DIRECTORS_CUT).await;
    assert!(!outputs(&app, DIRECTORS_CUT).await.is_empty());

    let status = delete(&app, &uri(DIRECTORS_CUT)).await;

    assert_eq!(status, 204);
    assert_eq!(outputs(&app, DIRECTORS_CUT).await, Vec::<String>::new());
    assert_eq!(app.get(&uri(DIRECTORS_CUT)).await.0, 404);
    assert_eq!(labels(&app).await, vec!["matrix"]);
    assert_eq!(
        outputs(&app, MATRIX).await.len(),
        3,
        "the other output stays"
    );
}

#[tokio::test]
async fn a_removed_media_file_is_not_placed_again_by_the_reconciler() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    upload_ready(&app, DIRECTORS_CUT).await;

    delete(&app, &uri(DIRECTORS_CUT)).await;
    app.reconcile().await;

    assert_eq!(labels(&app).await, vec!["matrix"]);
}

#[tokio::test]
async fn removing_an_unknown_media_file_is_not_found() {
    let app = TestApp::start().await;

    assert_eq!(delete(&app, &uri(MATRIX)).await, 404);
}

#[tokio::test]
async fn removing_the_default_makes_the_oldest_remaining_ready_one_the_default() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    app.advance(MINUTE);
    upload_ready(&app, DIRECTORS_CUT).await;
    app.advance(MINUTE);
    upload_ready(&app, EXTENDED).await;

    delete(&app, &uri(MATRIX)).await;

    assert_eq!(
        movie_default(&app).await.as_deref(),
        Some("matrix-directors-cut")
    );
}

#[tokio::test]
async fn removing_the_default_skips_media_files_that_are_not_ready() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    app.advance(MINUTE);
    app.upload(DIRECTORS_CUT).await;
    app.advance(MINUTE);
    upload_ready(&app, EXTENDED).await;

    delete(&app, &uri(MATRIX)).await;

    assert_eq!(
        movie_default(&app).await.as_deref(),
        Some("matrix-extended")
    );
}

#[tokio::test]
async fn removing_the_default_with_nothing_else_ready_leaves_the_default_to_the_next_one() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    app.upload(DIRECTORS_CUT).await;

    delete(&app, &uri(MATRIX)).await;
    app.finish_transcode(DIRECTORS_CUT).await;
    app.reconcile().await;

    assert_eq!(
        movie_default(&app).await.as_deref(),
        Some("matrix-directors-cut")
    );
}

#[tokio::test]
async fn a_movie_whose_last_media_file_was_removed_is_hidden_and_comes_back_with_its_metadata() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    upload_ready(&app, MATRIX).await;
    app.match_metadata().await;
    let calls = app.tmdb.calls();

    delete(&app, &uri(MATRIX)).await;
    assert_eq!(app.get_json("/movies").await, json!([]));

    upload_ready(&app, DIRECTORS_CUT).await;
    app.match_metadata().await;

    let listed = &app.get_json("/movies").await[0];
    assert_eq!(listed["tmdb_id"], json!(603));
    assert_eq!(listed["description"], json!("About The Matrix."));
    assert_eq!(default_of(listed).as_deref(), Some("matrix-directors-cut"));
    assert_eq!(app.tmdb.calls(), calls, "no new lookup: the Movie was kept");
}

// --- deleting Hidden entries ------------------------------------------------

const PILOT_2: &str = "shows/Breaking Bad (2008)/Season 01/S01E02.mkv";
const SEASON_2: &str = "shows/Breaking Bad (2008)/Season 02/S02E01.mkv";

#[tokio::test]
async fn deleting_a_hidden_movie_removes_it_for_good() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    upload_ready(&app, MATRIX).await;
    app.match_metadata().await;
    let movie_id = app.get_json("/movies").await[0]["id"].clone();
    delete(&app, &uri(MATRIX)).await;
    let calls = app.tmdb.calls();

    let status = delete(&app, &format!("/admin/movies/{movie_id}")).await;

    assert_eq!(status, 204);
    assert_eq!(
        delete(&app, &format!("/admin/movies/{movie_id}")).await,
        404
    );
    // A new upload to the same folder is a new Movie, looked up again.
    upload_ready(&app, MATRIX).await;
    app.match_metadata().await;
    assert!(app.tmdb.calls() > calls);
}

#[tokio::test]
async fn deleting_a_visible_movie_is_refused() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    let movie_id = app.get_json("/movies").await[0]["id"].clone();

    let status = delete(&app, &format!("/admin/movies/{movie_id}")).await;

    assert_eq!(status, 409);
    assert_eq!(labels(&app).await, vec!["matrix"]);
}

#[tokio::test]
async fn deleting_a_hidden_movie_removes_its_media_files_and_their_output() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    let movie_id = app.get_json("/movies").await[0]["id"].clone();
    lose_output(&app, MATRIX).await;
    app.reconcile().await; // Missing: the Movie is Hidden
    app.put(
        &app.buckets.final_,
        &format!("{}/hls/leftover.ts", id(MATRIX)),
        b"x",
    )
    .await;

    let status = delete(&app, &format!("/admin/movies/{movie_id}")).await;

    assert_eq!(status, 204);
    assert_eq!(app.get(&uri(MATRIX)).await.0, 404);
    assert_eq!(outputs(&app, MATRIX).await, Vec::<String>::new());
}

/// The id of the first listed Show.
async fn show_id(app: &TestApp) -> Value {
    app.get_json("/shows").await[0]["id"].clone()
}

#[tokio::test]
async fn deleting_a_hidden_episode_of_a_visible_season() {
    let app = TestApp::start().await;
    upload_ready(&app, PILOT).await;
    app.upload(PILOT_2).await;
    app.fail_transcode(PILOT_2, "ffmpeg exited with 1").await;
    let show = show_id(&app).await;
    let episode = |n: u32| format!("/admin/shows/{show}/seasons/1/episodes/{n}");

    assert_eq!(delete(&app, &episode(1)).await, 409);
    assert_eq!(delete(&app, &episode(2)).await, 204);
    assert_eq!(delete(&app, &episode(2)).await, 404);
    assert_eq!(app.get(&uri(PILOT_2)).await.0, 404);
    assert_eq!(app.get(&uri(PILOT)).await.0, 200);
}

#[tokio::test]
async fn deleting_a_hidden_season_of_a_visible_show() {
    let app = TestApp::start().await;
    upload_ready(&app, PILOT).await;
    app.upload(SEASON_2).await;
    let show = show_id(&app).await;

    assert_eq!(
        delete(&app, &format!("/admin/shows/{show}/seasons/1")).await,
        409
    );
    assert_eq!(
        delete(&app, &format!("/admin/shows/{show}/seasons/2")).await,
        204
    );
    assert_eq!(app.get(&uri(SEASON_2)).await.0, 404);
    assert_eq!(
        app.get_json(&format!("/shows/{show}/seasons")).await,
        json!([{ "number": 1 }])
    );
}

#[tokio::test]
async fn deleting_a_hidden_show_removes_its_seasons_episodes_and_media_files() {
    let app = TestApp::start().await;
    upload_ready(&app, PILOT).await;
    app.upload(SEASON_2).await;
    let show = show_id(&app).await;
    assert_eq!(delete(&app, &format!("/admin/shows/{show}")).await, 409);

    delete(&app, &uri(PILOT)).await;
    let status = delete(&app, &format!("/admin/shows/{show}")).await;

    assert_eq!(status, 204);
    assert_eq!(app.get(&uri(SEASON_2)).await.0, 404);
    assert_eq!(delete(&app, &format!("/admin/shows/{show}")).await, 404);
}
