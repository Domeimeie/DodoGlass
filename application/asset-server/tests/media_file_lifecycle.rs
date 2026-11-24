//! The Media file lifecycle: Failed, Missing, retries, and the status hints
//! that trigger an early reconcile (ADR 0001: the output marker decides Ready).
//!
//! Failed and Missing Media files are not in the read API; their state is
//! read from `GET /admin/media-files/<id>`.

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

// --- status hints -----------------------------------------------------------

#[tokio::test]
async fn a_failed_status_makes_the_media_file_failed_with_its_error() {
    let app = TestApp::start().await;
    app.upload(MATRIX).await;

    app.fail_transcode(MATRIX, "ffprobe: invalid data").await;

    assert_eq!(
        app.get_json(&status_uri()).await,
        json!({
            "id": MATRIX_ID,
            "key": MATRIX,
            "label": "matrix",
            "state": "failed",
            "error": "ffprobe: invalid data",
        })
    );
    assert_eq!(app.get_json("/movies").await, json!([]));
}

#[tokio::test]
async fn a_completed_status_makes_it_ready_without_waiting_for_the_periodic_reconcile() {
    let app = TestApp::start().await;
    app.upload(MATRIX).await;
    app.finish_transcode(MATRIX).await;

    app.send_completed(MATRIX).await;

    let movies = app.get_json("/movies").await;
    assert_eq!(movies[0]["media_files"][0]["id"], MATRIX_ID);
}

#[tokio::test]
async fn a_completed_status_without_an_output_marker_does_not_make_it_ready() {
    let app = TestApp::start().await;
    app.upload(MATRIX).await;

    app.send_completed(MATRIX).await;

    assert_eq!(app.get_json("/movies").await, json!([]));
    assert_eq!(app.get_json(&status_uri()).await["state"], "processing");
}

#[tokio::test]
async fn losing_every_status_message_still_ends_in_ready_after_a_reconcile() {
    let app = TestApp::start().await;
    app.upload(MATRIX).await;
    app.finish_transcode(MATRIX).await;

    app.reconcile().await;

    assert_eq!(app.get_json(&status_uri()).await["state"], "ready");
    assert_eq!(app.get_json("/movies").await.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn other_status_events_and_unreadable_messages_change_nothing() {
    let app = TestApp::start().await;
    app.upload(MATRIX).await;

    app.send_status(common::status_message(
        MATRIX,
        "job_failed",
        Some("HLS failed"),
    ))
    .await;
    app.send_status(json!("not a status message")).await;

    assert_eq!(app.get_json(&status_uri()).await["state"], "processing");
}

#[tokio::test]
async fn a_late_failed_status_does_not_undo_a_ready_media_file() {
    let app = TestApp::start().await;
    app.upload(MATRIX).await;
    app.finish_transcode(MATRIX).await;
    app.reconcile().await;

    app.fail_transcode(MATRIX, "cancelled").await;

    assert_eq!(app.get_json(&status_uri()).await["state"], "ready");
}

#[tokio::test]
async fn a_failed_media_file_whose_output_marker_appears_is_ready() {
    let app = TestApp::start().await;
    app.upload(MATRIX).await;
    app.fail_transcode(MATRIX, "worker stopped").await;

    // The task was delivered again, and another worker finished it.
    app.finish_transcode(MATRIX).await;
    app.reconcile().await;

    assert_eq!(app.get_json(&status_uri()).await["state"], "ready");
}

// --- Processing for too long ------------------------------------------------

#[tokio::test]
async fn processing_for_more_than_24_hours_with_the_input_still_in_ingest_is_failed() {
    let app = TestApp::start().await;
    app.put(&app.buckets.ingest, MATRIX, b"video").await;
    app.upload(MATRIX).await;

    app.advance(24 * HOUR + Duration::from_secs(1));
    app.reconcile().await;

    assert_eq!(
        app.get_json(&status_uri()).await,
        json!({
            "id": MATRIX_ID,
            "key": MATRIX,
            "label": "matrix",
            "state": "failed",
            "error": "no output after 24 h while the input is still in ingest",
        })
    );
}

#[tokio::test]
async fn an_nfd_upload_processing_for_more_than_24_hours_with_its_input_in_ingest_is_failed() {
    let app = TestApp::start().await;
    // Uploaded from a Mac: the key's bytes are NFD, and stay so in `ingest`.
    let nfd = "movies/Ame\u{301}lie (2001)/amelie.mkv";
    app.put(&app.buckets.ingest, nfd, b"video").await;
    app.upload(nfd).await;

    app.advance(24 * HOUR + Duration::from_secs(1));
    app.reconcile().await;

    let status = app
        .get_json("/admin/media-files/movies_Am_lie__2001__amelie-7657ef7e")
        .await;
    assert_eq!(status["state"], "failed");
    assert_eq!(status["key"], nfd);
}

#[tokio::test]
async fn processing_for_less_than_24_hours_is_not_failed() {
    let app = TestApp::start().await;
    app.put(&app.buckets.ingest, MATRIX, b"video").await;
    app.upload(MATRIX).await;

    app.advance(23 * HOUR);
    app.reconcile().await;

    assert_eq!(app.get_json(&status_uri()).await["state"], "processing");
}

#[tokio::test]
async fn processing_for_more_than_24_hours_with_the_input_gone_is_not_failed() {
    let app = TestApp::start().await;
    // The transcoder deletes the input once done: the marker is still to come.
    app.upload(MATRIX).await;

    app.advance(25 * HOUR);
    app.reconcile().await;
    assert_eq!(app.get_json(&status_uri()).await["state"], "processing");

    app.finish_transcode(MATRIX).await;
    app.reconcile().await;
    assert_eq!(app.get_json("/movies").await.as_array().unwrap().len(), 1);
}

// --- Missing ----------------------------------------------------------------

#[tokio::test]
async fn a_ready_media_file_whose_output_disappears_is_missing_until_it_comes_back() {
    let app = TestApp::start().await;
    app.upload(MATRIX).await;
    app.finish_transcode(MATRIX).await;
    app.reconcile().await;

    app.delete_prefix(&app.buckets.final_, MATRIX_ID).await;
    app.reconcile().await;
    assert_eq!(app.get_json(&status_uri()).await["state"], "missing");
    assert_eq!(app.get_json("/movies").await, json!([]));

    app.finish_transcode(MATRIX).await;
    app.reconcile().await;
    assert_eq!(app.get_json(&status_uri()).await["state"], "ready");
    let movies = app.get_json("/movies").await;
    assert_eq!(movies[0]["media_files"][0]["id"], MATRIX_ID);
}

#[tokio::test]
async fn a_missing_media_file_is_never_deleted() {
    let app = TestApp::start().await;
    app.upload(MATRIX).await;
    app.finish_transcode(MATRIX).await;
    app.reconcile().await;
    app.delete_prefix(&app.buckets.final_, MATRIX_ID).await;

    app.advance(30 * 24 * HOUR);
    app.reconcile().await;
    let app = app.restart().await;

    assert_eq!(app.get_json(&status_uri()).await["state"], "missing");
}

// --- retry ------------------------------------------------------------------

#[tokio::test]
async fn uploading_a_failed_media_file_again_makes_it_processing() {
    let app = TestApp::start().await;
    app.upload(MATRIX).await;
    app.fail_transcode(MATRIX, "ffprobe: invalid data").await;

    app.upload(MATRIX).await;

    assert_eq!(
        app.get_json(&status_uri()).await,
        json!({
            "id": MATRIX_ID,
            "key": MATRIX,
            "label": "matrix",
            "state": "processing",
            "error": null,
        })
    );
    app.finish_transcode(MATRIX).await;
    app.send_completed(MATRIX).await;
    assert_eq!(app.get_json("/movies").await.as_array().unwrap().len(), 1);
}

// --- unseen outputs ---------------------------------------------------------

#[tokio::test]
async fn an_output_marker_with_no_known_media_file_is_placed_as_ready() {
    let app = TestApp::start().await;

    // Uploaded while the asset server was not listening: no event, only output.
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
async fn an_unseen_episode_output_is_placed_in_its_show() {
    let app = TestApp::start().await;

    app.finish_transcode("shows/Breaking Bad (2008)/Season 01/S01E02.mkv")
        .await;
    app.reconcile().await;

    assert_eq!(app.get_json("/shows").await[0]["name"], "Breaking Bad");
}

#[tokio::test]
async fn a_completed_status_for_an_unknown_media_file_places_it_from_its_marker() {
    let app = TestApp::start().await;
    app.finish_transcode(MATRIX).await;

    app.send_completed(MATRIX).await;

    assert_eq!(app.get_json("/movies").await[0]["name"], "The Matrix");
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

    assert_eq!(app.get_json("/movies").await, json!([]));
}

#[tokio::test]
async fn an_unknown_media_file_has_no_admin_status() {
    let app = TestApp::start().await;

    let (status, _) = app.get(&status_uri()).await;

    assert_eq!(status, 404);
}
