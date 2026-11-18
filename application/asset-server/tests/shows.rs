//! Shows, Seasons and Episodes: from an upload in `shows/<Name> (<Year>)/Season <n>/`
//! to entries in `GET /shows`, `GET /shows/{id}/seasons` and
//! `GET /shows/{id}/seasons/{number}/episodes`.

mod common;

use common::TestApp;
use serde_json::{Value, json};

const S01E02: &str = "shows/Breaking Bad (2008)/Season 01/S01E02.mkv";
/// The task id of `S01E02` (ADR 0004), which is also its output folder in `final`.
const S01E02_ID: &str = "shows_Breaking_Bad__2008__Season_01_S01E02-bd50a00f";

/// Uploads `key` and lets its transcode finish and be found.
async fn ready(app: &TestApp, key: &str) {
    app.upload(key).await;
    app.finish_transcode(key).await;
    app.reconcile().await;
}

/// The id of the only listed Show.
async fn only_show_id(app: &TestApp) -> Value {
    let shows = app.get_json("/shows").await;
    assert_eq!(shows.as_array().unwrap().len(), 1, "{shows}");
    shows[0]["id"].clone()
}

#[tokio::test]
async fn an_episode_upload_builds_its_show_season_and_episode() {
    let app = TestApp::start().await;

    ready(&app, S01E02).await;

    let show = only_show_id(&app).await;
    assert_eq!(
        app.get_json("/shows").await,
        json!([{ "id": show, "name": "Breaking Bad", "year": 2008 }])
    );
    assert_eq!(
        app.get_json(&format!("/shows/{show}/seasons")).await,
        json!([{ "number": 1 }])
    );
    assert_eq!(
        app.get_json(&format!("/shows/{show}/seasons/1/episodes"))
            .await,
        json!([{
            "number": 2,
            "media_files": [{
                "id": S01E02_ID,
                "label": "S01E02",
                "default": true,
                "manifests": {
                    "hls": format!("final/{S01E02_ID}/hls/master.m3u8"),
                    "dash": format!("final/{S01E02_ID}/dash/manifest.mpd"),
                },
            }],
        }])
    );
}

#[tokio::test]
async fn specials_are_season_0() {
    let app = TestApp::start().await;

    ready(&app, "shows/Breaking Bad (2008)/Specials/S00E01.mkv").await;

    let show = only_show_id(&app).await;
    assert_eq!(
        app.get_json(&format!("/shows/{show}/seasons")).await,
        json!([{ "number": 0 }])
    );
    let episodes = app
        .get_json(&format!("/shows/{show}/seasons/0/episodes"))
        .await;
    assert_eq!(episodes[0]["number"], 1);
}

#[tokio::test]
async fn episodes_of_one_show_and_season_share_them() {
    let app = TestApp::start().await;

    ready(
        &app,
        "shows/Breaking Bad (2008)/Season 01/S01E01 - Pilot.mkv",
    )
    .await;
    ready(&app, S01E02).await;
    ready(&app, "shows/Breaking Bad (2008)/Season 2/S02E01.mp4").await;

    let show = only_show_id(&app).await;
    assert_eq!(
        app.get_json(&format!("/shows/{show}/seasons")).await,
        json!([{ "number": 1 }, { "number": 2 }])
    );
    let episodes = app
        .get_json(&format!("/shows/{show}/seasons/1/episodes"))
        .await;
    assert_eq!(episodes[0]["number"], 1);
    assert_eq!(episodes[0]["media_files"][0]["label"], "S01E01 - Pilot");
    assert_eq!(episodes[1]["number"], 2);
    assert_eq!(episodes.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn shows_are_told_apart_by_name_and_year() {
    let app = TestApp::start().await;

    ready(&app, "shows/Doctor Who (2005)/Season 1/S01E01.mkv").await;
    ready(&app, "shows/Doctor Who (1963)/Season 1/S01E01.mkv").await;
    ready(&app, "shows/Dark (2017)/Season 1/S01E01.mkv").await;

    let shows = app.get_json("/shows").await;
    let names: Vec<(Value, Value)> = shows
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].clone(), s["year"].clone()))
        .collect();
    assert_eq!(
        names,
        [
            (json!("Dark"), json!(2017)),
            (json!("Doctor Who"), json!(1963)),
            (json!("Doctor Who"), json!(2005)),
        ]
    );
}

#[tokio::test]
async fn an_episode_still_processing_is_hidden_and_so_are_its_season_and_show() {
    let app = TestApp::start().await;

    app.upload(S01E02).await;
    app.reconcile().await;

    assert_eq!(app.get_json("/shows").await, json!([]));
}

#[tokio::test]
async fn a_season_without_a_visible_episode_is_hidden() {
    let app = TestApp::start().await;

    ready(&app, S01E02).await;
    app.upload("shows/Breaking Bad (2008)/Season 02/S02E01.mkv")
        .await;
    app.reconcile().await;

    let show = only_show_id(&app).await;
    assert_eq!(
        app.get_json(&format!("/shows/{show}/seasons")).await,
        json!([{ "number": 1 }])
    );
    let (status, _) = app.get(&format!("/shows/{show}/seasons/2/episodes")).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn an_episode_without_a_ready_media_file_is_not_listed() {
    let app = TestApp::start().await;

    ready(&app, S01E02).await;
    app.upload("shows/Breaking Bad (2008)/Season 01/S01E03.mkv")
        .await;
    app.reconcile().await;

    let show = only_show_id(&app).await;
    let episodes = app
        .get_json(&format!("/shows/{show}/seasons/1/episodes"))
        .await;
    assert_eq!(episodes.as_array().unwrap().len(), 1);
    assert_eq!(episodes[0]["number"], 2);
}

#[tokio::test]
async fn a_hidden_or_unknown_show_has_no_seasons() {
    let app = TestApp::start().await;

    app.upload(S01E02).await;

    let (status, body) = app.get("/shows/1/seasons").await;
    assert_eq!(status, 404);
    assert_eq!(body, json!({ "error": "no such show" }));
    let (status, _) = app.get("/shows/1/seasons/1/episodes").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn episodes_are_not_listed_as_movies_nor_movies_as_shows() {
    let app = TestApp::start().await;

    ready(&app, S01E02).await;
    ready(&app, "movies/The Matrix (1999)/matrix.mkv").await;

    let movies = app.get_json("/movies").await;
    assert_eq!(movies.as_array().unwrap().len(), 1, "{movies}");
    assert_eq!(movies[0]["name"], "The Matrix");
    let shows = app.get_json("/shows").await;
    assert_eq!(shows.as_array().unwrap().len(), 1, "{shows}");
    assert_eq!(shows[0]["name"], "Breaking Bad");
}

#[tokio::test]
async fn an_episode_upload_outside_the_layout_builds_nothing() {
    let app = TestApp::start().await;

    // The season in the file name disagrees with the folder: Unassigned.
    ready(&app, "shows/Breaking Bad (2008)/Season 01/S02E01.mkv").await;

    assert_eq!(app.get_json("/shows").await, json!([]));
}
