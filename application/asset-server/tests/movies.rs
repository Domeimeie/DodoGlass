//! Movies: from an upload in `movies/<Name> (<Year>)/` to an entry in `GET /movies`.

mod common;

use common::TestApp;
use serde_json::json;

const MATRIX: &str = "movies/The Matrix (1999)/matrix.mkv";
/// The task id of `MATRIX` (ADR 0004), which is also its output folder in `final`.
const MATRIX_ID: &str = "movies_The_Matrix__1999__matrix-48c3476c";

#[tokio::test]
async fn a_movie_upload_is_listed_once_its_transcode_is_ready() {
    let app = TestApp::start().await;

    app.upload(MATRIX).await;
    app.finish_transcode(MATRIX).await;
    app.reconcile().await;

    assert_eq!(
        app.get_json("/movies").await,
        json!([{
            "name": "The Matrix",
            "year": 1999,
            "media_files": [{
                "id": MATRIX_ID,
                "label": "matrix",
                "manifests": {
                    "hls": format!("final/{MATRIX_ID}/hls/master.m3u8"),
                    "dash": format!("final/{MATRIX_ID}/dash/manifest.mpd"),
                },
            }],
        }])
    );
}
