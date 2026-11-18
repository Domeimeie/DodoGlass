//! Several Media files per Movie: Labels, the Default media file, and
//! re-uploads to the same path.

mod common;

use common::TestApp;
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
