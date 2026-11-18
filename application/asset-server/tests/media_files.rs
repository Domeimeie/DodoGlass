//! Several Media files per Movie or Episode: Labels, the Default media file, and
//! re-uploads to the same path.

mod common;

use common::TestApp;
use object_store::ObjectStoreExt;
use serde_json::{Value, json};

const MATRIX: &str = "movies/The Matrix (1999)/matrix.mkv";
const DIRECTORS_CUT: &str = "movies/The Matrix (1999)/matrix-directors-cut.mkv";

/// `(label, default)` of each Media file of the first listed Movie.
fn labels(movies: &Value) -> Vec<(String, bool)> {
    movies[0]["media_files"]
        .as_array()
        .expect("media_files is a list")
        .iter()
        .map(|f| {
            (
                f["label"].as_str().expect("label").to_string(),
                f["default"].as_bool().expect("default"),
            )
        })
        .collect()
}

async fn upload_ready(app: &TestApp, key: &str) {
    app.upload(key).await;
    app.finish_transcode(key).await;
}

#[tokio::test]
async fn two_uploads_in_one_movie_folder_are_one_movie_with_two_labelled_media_files() {
    let app = TestApp::start().await;

    upload_ready(&app, MATRIX).await;
    upload_ready(&app, DIRECTORS_CUT).await;
    app.reconcile().await;

    let movies = app.get_json("/movies").await;
    assert_eq!(movies.as_array().unwrap().len(), 1);
    assert_eq!(
        labels(&movies),
        vec![
            ("matrix".to_string(), true),
            ("matrix-directors-cut".to_string(), false),
        ]
    );
}

#[tokio::test]
async fn a_label_taken_in_the_same_movie_gets_the_extension_added() {
    let app = TestApp::start().await;

    upload_ready(&app, MATRIX).await;
    upload_ready(&app, "movies/The Matrix (1999)/matrix.mp4").await;
    upload_ready(&app, "movies/The Matrix (1999)/matrix (mp4).mkv").await;
    app.reconcile().await;

    assert_eq!(
        labels(&app.get_json("/movies").await),
        vec![
            ("matrix".to_string(), true),
            ("matrix (mp4)".to_string(), false),
            // Its file name gives "matrix (mp4)", taken by then.
            ("matrix (mp4) (mkv)".to_string(), false),
        ]
    );
}

#[tokio::test]
async fn the_same_label_in_two_movies_is_no_collision() {
    let app = TestApp::start().await;

    upload_ready(&app, MATRIX).await;
    upload_ready(&app, "movies/The Matrix Reloaded (2003)/matrix.mp4").await;
    app.reconcile().await;

    let movies = app.get_json("/movies").await;
    assert_eq!(movies[0]["media_files"][0]["label"], "matrix");
    assert_eq!(movies[0]["media_files"][0]["default"], true);
    assert_eq!(movies[1]["media_files"][0]["label"], "matrix");
    assert_eq!(movies[1]["media_files"][0]["default"], true);
}

#[tokio::test]
async fn only_ready_media_files_are_listed() {
    let app = TestApp::start().await;

    upload_ready(&app, MATRIX).await;
    app.upload(DIRECTORS_CUT).await;
    app.reconcile().await;

    assert_eq!(
        labels(&app.get_json("/movies").await),
        vec![("matrix".to_string(), true)]
    );
}

#[tokio::test]
async fn the_default_is_the_first_upload_even_if_another_is_ready_first() {
    let app = TestApp::start().await;

    app.upload(MATRIX).await;
    upload_ready(&app, DIRECTORS_CUT).await;
    app.reconcile().await;
    assert_eq!(
        labels(&app.get_json("/movies").await),
        vec![("matrix-directors-cut".to_string(), false)]
    );

    app.finish_transcode(MATRIX).await;
    app.reconcile().await;
    assert_eq!(
        labels(&app.get_json("/movies").await),
        vec![
            ("matrix".to_string(), true),
            ("matrix-directors-cut".to_string(), false),
        ]
    );
}

#[tokio::test]
async fn a_re_upload_keeps_the_media_file_and_is_processing_until_its_new_output_is_complete() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    upload_ready(&app, "movies/The Matrix (1999)/matrix.mp4").await;
    app.reconcile().await;
    let before = app.get_json("/movies").await[0]["media_files"].clone();
    assert_eq!(
        before,
        json!([
            {
                "id": before[0]["id"],
                "label": "matrix",
                "default": true,
                "manifests": before[0]["manifests"],
            },
            {
                "id": before[1]["id"],
                "label": "matrix (mp4)",
                "default": false,
                "manifests": before[1]["manifests"],
            },
        ])
    );

    // A new version of `matrix.mkv`: the transcoder replaces its output, so
    // it is Processing until the new `source.json` appears.
    let marker = format!("{}/source.json", before[0]["id"].as_str().unwrap());
    app.buckets.final_.delete(&marker.into()).await.unwrap();
    app.upload(MATRIX).await;
    app.reconcile().await;
    assert_eq!(
        labels(&app.get_json("/movies").await),
        vec![("matrix (mp4)".to_string(), false)]
    );

    app.finish_transcode(MATRIX).await;
    app.reconcile().await;
    assert_eq!(app.get_json("/movies").await[0]["media_files"], before);
}

#[tokio::test]
async fn uploading_the_same_path_again_makes_no_duplicate() {
    let app = TestApp::start().await;
    upload_ready(&app, MATRIX).await;
    app.reconcile().await;

    app.upload(MATRIX).await;
    app.upload(MATRIX).await;
    app.reconcile().await;

    let movies = app.get_json("/movies").await;
    assert_eq!(movies.as_array().unwrap().len(), 1);
    assert_eq!(labels(&movies), vec![("matrix".to_string(), true)]);
}

const S01E02: &str = "shows/Breaking Bad (2008)/Season 01/S01E02.mkv";

/// The Episodes of Season 1 of the only listed Show.
async fn season_1(app: &TestApp) -> Value {
    let show = &app.get_json("/shows").await[0]["id"];
    app.get_json(&format!("/shows/{show}/seasons/1/episodes"))
        .await
}

#[tokio::test]
async fn an_episode_has_labelled_media_files_and_a_default_like_a_movie() {
    let app = TestApp::start().await;

    upload_ready(&app, S01E02).await;
    upload_ready(&app, "shows/Breaking Bad (2008)/Season 01/S01E02.mp4").await;
    upload_ready(
        &app,
        "shows/Breaking Bad (2008)/Season 01/S01E02 - extended.mkv",
    )
    .await;
    // The same Label in another Episode is no collision.
    upload_ready(&app, "shows/Breaking Bad (2008)/Season 01/S01E03.mkv").await;
    app.reconcile().await;

    let episodes = season_1(&app).await;
    assert_eq!(
        labels(&episodes),
        vec![
            ("S01E02".to_string(), true),
            ("S01E02 (mp4)".to_string(), false),
            ("S01E02 - extended".to_string(), false),
        ]
    );
    assert_eq!(episodes[1]["media_files"][0]["label"], "S01E03");
    assert_eq!(episodes[1]["media_files"][0]["default"], true);
}

#[tokio::test]
async fn a_re_upload_of_an_episode_keeps_its_media_file() {
    let app = TestApp::start().await;
    upload_ready(&app, S01E02).await;
    upload_ready(&app, "shows/Breaking Bad (2008)/Season 01/S01E02.mp4").await;
    app.reconcile().await;
    let before = season_1(&app).await;

    let marker = format!(
        "{}/source.json",
        before[0]["media_files"][0]["id"].as_str().unwrap()
    );
    app.buckets.final_.delete(&marker.into()).await.unwrap();
    app.upload(S01E02).await;
    app.reconcile().await;
    assert_eq!(
        labels(&season_1(&app).await),
        vec![("S01E02 (mp4)".to_string(), false)]
    );

    app.finish_transcode(S01E02).await;
    app.reconcile().await;
    assert_eq!(season_1(&app).await, before);
}
