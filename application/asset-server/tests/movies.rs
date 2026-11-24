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
            "id": 1,
            "name": "The Matrix",
            "year": 1999,
            "rating": null,
            "votes": null,
            "rating_updated_at": null,
            "media_files": [{
                "id": MATRIX_ID,
                "label": "matrix",
                "default": true,
                "manifests": {
                    "hls": format!("final/{MATRIX_ID}/hls/master.m3u8"),
                    "dash": format!("final/{MATRIX_ID}/dash/manifest.mpd"),
                },
            }],
        }])
    );
}

#[tokio::test]
async fn a_movie_still_processing_is_hidden() {
    let app = TestApp::start().await;

    app.upload(MATRIX).await;
    app.reconcile().await;

    assert_eq!(app.get_json("/movies").await, json!([]));
}

#[tokio::test]
async fn the_output_marker_counts_only_once_a_reconcile_has_run() {
    let app = TestApp::start().await;

    app.upload(MATRIX).await;
    app.finish_transcode(MATRIX).await;
    assert_eq!(app.get_json("/movies").await, json!([]));

    app.reconcile().await;
    assert_eq!(app.get_json("/movies").await.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_transcode_finished_while_the_asset_server_was_down_is_found_at_startup() {
    let app = TestApp::start().await;
    app.upload(MATRIX).await;

    app.finish_transcode(MATRIX).await;
    let app = app.restart().await;

    let movies = app.get_json("/movies").await;
    assert_eq!(movies[0]["name"], "The Matrix");
    assert_eq!(movies[0]["media_files"][0]["id"], MATRIX_ID);
}

#[tokio::test]
async fn an_upload_outside_the_layout_is_not_listed() {
    let app = TestApp::start().await;

    // The fixture as RustFS sent it: `sample.mp4` at the root of `ingest`.
    app.send_event(common::rustfs_event()).await;
    app.finish_transcode("sample.mp4").await;
    app.reconcile().await;

    assert_eq!(app.get_json("/movies").await, json!([]));
}

#[tokio::test]
async fn every_record_of_a_message_is_placed_with_its_key_decoded() {
    let app = TestApp::start().await;
    let amelie = "movies/Amélie (2001)/amélie [director's cut]+extra.mkv";

    app.send_event(common::s3_event(&[MATRIX, amelie])).await;
    app.finish_transcode(MATRIX).await;
    app.finish_transcode(amelie).await;
    app.reconcile().await;

    let movies = app.get_json("/movies").await;
    assert_eq!(movies[0]["name"], "Amélie");
    assert_eq!(movies[0]["year"], 2001);
    assert_eq!(
        movies[0]["media_files"][0]["label"],
        "amélie [director's cut]+extra"
    );
    assert_eq!(movies[1]["name"], "The Matrix");
}

#[tokio::test]
async fn nfc_and_nfd_spellings_of_a_path_are_the_same_movie() {
    let app = TestApp::start().await;
    let nfc = "movies/Am\u{e9}lie (2001)/amelie.mkv";
    let nfd = "movies/Ame\u{301}lie (2001)/amelie.mkv";

    app.upload(nfd).await;
    app.finish_transcode(nfc).await;
    app.reconcile().await;

    assert_eq!(
        app.get_json("/movies").await,
        json!([{
            "id": 1,
            "name": "Am\u{e9}lie",
            "year": 2001,
            "rating": null,
            "votes": null,
            "rating_updated_at": null,
            "media_files": [{
                "id": "movies_Am_lie__2001__amelie-7657ef7e",
                "label": "amelie",
                "default": true,
                "manifests": {
                    "hls": "final/movies_Am_lie__2001__amelie-7657ef7e/hls/master.m3u8",
                    "dash": "final/movies_Am_lie__2001__amelie-7657ef7e/dash/manifest.mpd",
                },
            }],
        }])
    );
}
