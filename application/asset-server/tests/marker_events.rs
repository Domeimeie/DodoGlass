//! How a Media file gets into the catalogue (ADR 0006): RustFS publishes every
//! new output marker (`<id>/source.json`) in `final` on `dodoglass/final`, and
//! the asset server places the Media file as Ready straight away. The
//! reconcile stays as the safety net for lost events (ADR 0001).

mod common;

use common::TestApp;
use serde_json::json;

const MATRIX: &str = "movies/The Matrix (1999)/matrix.mkv";
/// The task id of `MATRIX` (ADR 0004).
const MATRIX_ID: &str = "movies_The_Matrix__1999__matrix-48c3476c";

#[tokio::test]
async fn a_marker_event_for_a_movie_path_lists_the_movie_with_a_ready_media_file() {
    let app = TestApp::start().await;

    app.finish_transcode(MATRIX).await;

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
async fn a_marker_event_for_an_episode_path_lists_its_show_season_and_episode() {
    let app = TestApp::start().await;

    app.finish_transcode("shows/Breaking Bad (2008)/Season 01/S01E02.mkv")
        .await;

    let shows = app.get_json("/shows").await;
    assert_eq!(shows[0]["name"], "Breaking Bad");
    let show = shows[0]["id"].as_i64().unwrap();
    let seasons = app.get_json(&format!("/shows/{show}/seasons")).await;
    assert_eq!(seasons[0]["number"], 1);
    let episodes = app
        .get_json(&format!("/shows/{show}/seasons/1/episodes"))
        .await;
    assert_eq!(episodes[0]["number"], 2);
    assert_eq!(episodes[0]["media_files"][0]["label"], "S01E02");
    assert_eq!(episodes[0]["media_files"][0]["default"], true);
}

#[tokio::test]
async fn a_marker_event_for_a_path_that_does_not_fit_makes_a_ready_unassigned_media_file() {
    let app = TestApp::start().await;

    app.finish_transcode("misc/clip.mp4").await;

    let unassigned = app.get_json("/admin/unassigned-media-files").await;
    assert_eq!(unassigned[0]["path"], "misc/clip.mp4");
    assert_eq!(unassigned[0]["state"], "ready");
}

#[tokio::test]
async fn the_same_marker_event_delivered_twice_changes_nothing_the_second_time() {
    let app = TestApp::start().await;
    app.finish_transcode(MATRIX).await;
    let once = app.get_json("/movies").await;

    app.send_marker_event(common::final_event(&[&marker_key()]))
        .await;

    assert_eq!(app.get_json("/movies").await, once);
    assert_eq!(app.get_json(&status_uri()).await["state"], "ready");
}

#[tokio::test]
async fn without_any_event_the_reconcile_still_places_the_media_file() {
    let app = TestApp::start().await;
    app.finish_transcode_without_event(MATRIX).await;
    assert_eq!(app.get_json("/movies").await, json!([]));

    app.reconcile().await;

    assert_eq!(app.get_json("/movies").await[0]["name"], "The Matrix");
}

#[tokio::test]
async fn events_for_other_objects_in_final_change_nothing() {
    let app = TestApp::start().await;
    app.finish_transcode_without_event(MATRIX).await;

    app.send_marker_event(common::final_event(&[
        &format!("{MATRIX_ID}/hls/master.m3u8"),
        &format!("{MATRIX_ID}/hls/source.json"),
    ]))
    .await;
    app.send_marker_event(b"not an S3 event".to_vec()).await;

    assert_eq!(app.get_json("/movies").await, json!([]));
}

#[tokio::test]
async fn an_unreadable_marker_is_acknowledged_and_places_nothing() {
    let app = TestApp::start().await;
    app.put(&app.buckets.final_, &marker_key(), b"{").await;

    app.send_marker_event(common::final_event(&[&marker_key()]))
        .await;

    assert_eq!(app.get_json("/movies").await, json!([]));
}

fn marker_key() -> String {
    format!("{MATRIX_ID}/source.json")
}

fn status_uri() -> String {
    format!("/admin/media-files/{MATRIX_ID}")
}
