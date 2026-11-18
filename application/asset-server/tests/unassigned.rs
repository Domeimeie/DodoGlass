//! Unassigned media files: uploads that do not follow the upload layout are
//! kept, listed in the admin API with the reason, and attached by an admin.

mod common;

use common::TestApp;
use serde_json::{Value, json};

const UNASSIGNED: &str = "/admin/unassigned-media-files";

/// The Unassigned media files, as the admin API lists them.
async fn unassigned(app: &TestApp) -> Value {
    app.get_json(UNASSIGNED).await
}

#[tokio::test]
async fn an_upload_without_a_year_is_listed_as_unassigned_and_processing() {
    let app = TestApp::start().await;

    app.upload("movies/The Matrix/matrix.mkv").await;

    let list = unassigned(&app).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list[0]["path"], "movies/The Matrix/matrix.mkv");
    assert_eq!(list[0]["reason"], "no_year");
    assert_eq!(list[0]["state"], "processing");
}

#[tokio::test]
async fn each_kind_of_upload_outside_the_layout_is_kept_with_its_reason() {
    let app = TestApp::start().await;

    app.send_event(common::s3_event(&[
        "misc/clip.mp4",
        "shows/Breaking Bad (2008)/Season 01/S02E01.mkv",
        "shows/Breaking Bad (2008)/Season 01/Pilot.mkv",
    ]))
    .await;

    let list = unassigned(&app).await;
    let reasons: Vec<(&str, &str)> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["path"].as_str().unwrap(), f["reason"].as_str().unwrap()))
        .collect();
    assert_eq!(
        reasons,
        [
            ("misc/clip.mp4", "unknown_root_folder"),
            (
                "shows/Breaking Bad (2008)/Season 01/Pilot.mkv",
                "no_episode_code"
            ),
            (
                "shows/Breaking Bad (2008)/Season 01/S02E01.mkv",
                "season_mismatch"
            ),
        ]
    );
}

#[tokio::test]
async fn an_unassigned_media_file_becomes_ready_but_is_not_in_the_read_api() {
    let app = TestApp::start().await;

    app.upload("movies/The Matrix/matrix.mkv").await;
    app.finish_transcode("movies/The Matrix/matrix.mkv").await;
    app.reconcile().await;

    assert_eq!(unassigned(&app).await[0]["state"], "ready");
    assert_eq!(app.get_json("/movies").await, json!([]));
    assert_eq!(app.get_json("/shows").await, json!([]));
}

#[tokio::test]
async fn placed_media_files_are_not_unassigned() {
    let app = TestApp::start().await;

    app.upload("movies/The Matrix (1999)/matrix.mkv").await;
    app.upload("shows/Breaking Bad (2008)/Season 01/S01E02.mkv")
        .await;

    assert_eq!(unassigned(&app).await, json!([]));
}
