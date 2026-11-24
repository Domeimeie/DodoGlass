//! The Media file lifecycle: Ready from its output marker, Missing while the
//! marker is gone, and outputs found by the reconcile alone (ADR 0001: the
//! output marker decides Ready; ADR 0006: only finished output is in the
//! catalogue).
//!
//! Missing Media files are not in the read API; their state is read from
//! `GET /admin/media-files/<id>`.

mod common;

use std::time::Duration;

use common::TestApp;
use serde_json::json;

const MATRIX: &str = "movies/The Matrix (1999)/matrix.mkv";
/// The task id of `MATRIX` (ADR 0004).
const MATRIX_ID: &str = "movies_The_Matrix__1999__matrix-48c3476c";

const HOUR: Duration = Duration::from_secs(3600);

fn status_uri() -> String {
    format!("/admin/media-files/{MATRIX_ID}")
}

#[tokio::test]
async fn a_finished_transcode_has_a_ready_admin_status() {
    let app = TestApp::start().await;

    app.finish_transcode(MATRIX).await;

    assert_eq!(
        app.get_json(&status_uri()).await,
        json!({
            "id": MATRIX_ID,
            "key": MATRIX,
            "label": "matrix",
            "state": "ready",
        })
    );
}

// --- Missing ----------------------------------------------------------------

#[tokio::test]
async fn a_ready_media_file_whose_output_disappears_is_missing_until_it_comes_back() {
    let app = TestApp::start().await;
    app.finish_transcode(MATRIX).await;

    app.delete_prefix(&app.buckets.final_, MATRIX_ID).await;
    app.reconcile().await;
    assert_eq!(app.get_json(&status_uri()).await["state"], "missing");
    assert_eq!(app.get_json("/movies").await, json!([]));

    app.finish_transcode(MATRIX).await;
    assert_eq!(app.get_json(&status_uri()).await["state"], "ready");
    let movies = app.get_json("/movies").await;
    assert_eq!(movies[0]["media_files"][0]["id"], MATRIX_ID);
}

#[tokio::test]
async fn a_re_upload_is_missing_while_transcoded_and_keeps_its_label() {
    let app = TestApp::start().await;
    app.finish_transcode(MATRIX).await;
    app.send_json("PATCH", &status_uri(), json!({ "label": "1080p" }))
        .await;

    app.start_retranscode(MATRIX).await;
    app.reconcile().await;
    assert_eq!(app.get_json(&status_uri()).await["state"], "missing");

    app.finish_transcode(MATRIX).await;
    let status = app.get_json(&status_uri()).await;
    assert_eq!(status["state"], "ready");
    assert_eq!(status["label"], "1080p");
}

#[tokio::test]
async fn a_marker_event_after_the_marker_is_gone_again_makes_it_missing_not_ready() {
    let app = TestApp::start().await;
    app.finish_transcode(MATRIX).await;
    app.start_retranscode(MATRIX).await;

    // The event of the old marker, delivered late (at least once).
    app.send_marker_event(common::final_event(&[&format!("{MATRIX_ID}/source.json")]))
        .await;

    assert_eq!(app.get_json(&status_uri()).await["state"], "missing");
}

#[tokio::test]
async fn a_missing_media_file_is_never_deleted() {
    let app = TestApp::start().await;
    app.finish_transcode(MATRIX).await;
    app.delete_prefix(&app.buckets.final_, MATRIX_ID).await;

    app.advance(30 * 24 * HOUR);
    app.reconcile().await;
    let app = app.restart().await;

    assert_eq!(app.get_json(&status_uri()).await["state"], "missing");
}

// --- outputs found by the reconcile -----------------------------------------

#[tokio::test]
async fn an_output_whose_event_was_lost_is_placed_as_ready_by_the_reconcile() {
    let app = TestApp::start().await;

    app.finish_transcode_without_event(MATRIX).await;
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
async fn an_episode_output_whose_event_was_lost_is_placed_in_its_show() {
    let app = TestApp::start().await;

    app.finish_transcode_without_event("shows/Breaking Bad (2008)/Season 01/S01E02.mkv")
        .await;
    app.reconcile().await;

    assert_eq!(app.get_json("/shows").await[0]["name"], "Breaking Bad");
}

#[tokio::test]
async fn a_folder_whose_marker_names_the_key_of_another_folder_is_not_placed() {
    let app = TestApp::start().await;
    let marker = json!({
        "input": { "bucket": "ingest", "key": MATRIX },
        "etag": "e1",
        "formats": ["hls"],
        "completed_ms": 1,
        "worker": "w",
    });
    app.put(
        &app.buckets.final_,
        "copied-folder/source.json",
        marker.to_string().as_bytes(),
    )
    .await;

    app.reconcile().await;
    app.send_marker_event(common::final_event(&["copied-folder/source.json"]))
        .await;

    assert_eq!(app.get_json("/movies").await, json!([]));
}

#[tokio::test]
async fn an_unknown_media_file_has_no_admin_status() {
    let app = TestApp::start().await;

    let (status, _) = app.get(&status_uri()).await;

    assert_eq!(status, 404);
}
