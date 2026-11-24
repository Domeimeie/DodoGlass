//! Unassigned media files: uploads that do not follow the upload layout are
//! kept, listed in the admin API with the reason, and attached by an admin.

mod common;

use common::TestApp;
use common::tmdb::{episode, movie, season, show};
use serde_json::{Value, json};

const UNASSIGNED: &str = "/admin/unassigned-media-files";

/// The Unassigned media files, as the admin API lists them.
async fn unassigned(app: &TestApp) -> Value {
    app.get_json(UNASSIGNED).await
}

#[tokio::test]
async fn an_upload_without_a_year_is_listed_as_unassigned_and_ready() {
    let app = TestApp::start().await;

    app.finish_transcode("movies/The Matrix/matrix.mkv").await;

    let list = unassigned(&app).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list[0]["path"], "movies/The Matrix/matrix.mkv");
    assert_eq!(list[0]["reason"], "no_year");
    assert_eq!(list[0]["state"], "ready");
}

#[tokio::test]
async fn an_unassigned_upload_is_listed_with_its_path_as_uploaded() {
    let app = TestApp::start().await;
    let nfd = "misc/Ame\u{301}lie.mkv";

    app.finish_transcode(nfd).await;

    assert_eq!(unassigned(&app).await[0]["path"], nfd);
}

#[tokio::test]
async fn each_kind_of_upload_outside_the_layout_is_kept_with_its_reason() {
    let app = TestApp::start().await;

    let keys = [
        "misc/clip.mp4",
        "shows/Breaking Bad (2008)/Season 01/S02E01.mkv",
        "shows/Breaking Bad (2008)/Season 01/Pilot.mkv",
    ];
    let mut markers = Vec::new();
    for key in keys {
        app.finish_transcode_without_event(key).await;
        markers.push(format!(
            "{}/source.json",
            dodoglass_ids::task_id_from_key(key)
        ));
    }
    let markers: Vec<&str> = markers.iter().map(String::as_str).collect();
    app.send_marker_event(common::final_event(&markers)).await;

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

    app.finish_transcode("movies/The Matrix/matrix.mkv").await;

    assert_eq!(unassigned(&app).await[0]["state"], "ready");
    assert_eq!(app.get_json("/movies").await, json!([]));
    assert_eq!(app.get_json("/shows").await, json!([]));
}

#[tokio::test]
async fn a_missing_unassigned_media_file_is_listed_as_missing() {
    let app = TestApp::start().await;

    app.finish_transcode("misc/clip.mp4").await;
    app.lose_output("misc/clip.mp4").await;

    assert_eq!(unassigned(&app).await[0]["state"], "missing");
}

#[tokio::test]
async fn an_output_outside_the_layout_found_in_final_is_unassigned() {
    let app = TestApp::start().await;

    // The marker event was lost: only the reconcile finds the output.
    app.finish_transcode_without_event("misc/clip.mp4").await;
    app.reconcile().await;

    let list = unassigned(&app).await;
    assert_eq!(list[0]["path"], "misc/clip.mp4");
    assert_eq!(list[0]["state"], "ready");
}

#[tokio::test]
async fn placed_media_files_are_not_unassigned() {
    let app = TestApp::start().await;

    app.finish_transcode("movies/The Matrix (1999)/matrix.mkv")
        .await;
    app.finish_transcode("shows/Breaking Bad (2008)/Season 01/S01E02.mkv")
        .await;

    assert_eq!(unassigned(&app).await, json!([]));
}

/// Uploads `key` and lets its transcode finish and be found.
async fn ready(app: &TestApp, key: &str) {
    app.finish_transcode(key).await;
}

/// The id of the only Unassigned media file.
async fn only_unassigned_id(app: &TestApp) -> String {
    let list = unassigned(app).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    list[0]["id"].as_str().unwrap().to_string()
}

async fn attach(app: &TestApp, id: &str, to: Value) -> (axum::http::StatusCode, Value) {
    app.send_json("POST", &format!("{UNASSIGNED}/{id}/attach"), to)
        .await
}

#[tokio::test]
async fn an_attached_media_file_is_listed_in_its_movie_once_ready() {
    let app = TestApp::start().await;
    // The Movie exists, Hidden: its only Media file is Missing. So is the
    // Unassigned one.
    app.finish_transcode("movies/Dune (2021)/dune.mkv").await;
    app.lose_output("movies/Dune (2021)/dune.mkv").await;
    app.finish_transcode("movies/Dune/dune-extended.mkv").await;
    app.lose_output("movies/Dune/dune-extended.mkv").await;
    let id = only_unassigned_id(&app).await;

    let (status, _) = attach(
        &app,
        &id,
        json!({ "movie": { "name": "Dune", "year": 2021 } }),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(unassigned(&app).await, json!([]));
    assert_eq!(app.get_json("/movies").await, json!([]));

    app.finish_transcode("movies/Dune/dune-extended.mkv").await;
    let movies = app.get_json("/movies").await;
    assert_eq!(movies[0]["name"], "Dune");
    assert_eq!(movies[0]["year"], 2021);
    assert_eq!(movies[0]["media_files"][0]["id"], id);
    assert_eq!(movies[0]["media_files"][0]["label"], "dune-extended");
}

#[tokio::test]
async fn an_attached_media_file_gets_a_label_free_in_its_movie() {
    let app = TestApp::start().await;
    ready(&app, "movies/The Matrix (1999)/matrix.mkv").await;
    ready(&app, "movies/The Matrix/matrix.mkv").await;
    let id = only_unassigned_id(&app).await;

    let (status, body) = attach(
        &app,
        &id,
        json!({ "movie": { "name": "The Matrix", "year": 1999 } }),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body, json!({ "id": id, "label": "matrix (mkv)" }));
    let files = &app.get_json("/movies").await[0]["media_files"];
    let labels: Vec<(&str, bool)> = files
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["label"].as_str().unwrap(),
                f["default"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(labels, [("matrix", true), ("matrix (mkv)", false)]);
}

#[tokio::test]
async fn an_attached_media_file_does_not_take_over_the_default() {
    let app = TestApp::start().await;
    // alien.mkv came first, so it stays the default, even while Missing.
    app.finish_transcode("movies/Alien (1979)/alien.mkv").await;
    app.lose_output("movies/Alien (1979)/alien.mkv").await;
    ready(&app, "movies/Alien/alien-remastered.mkv").await;
    let id = only_unassigned_id(&app).await;

    attach(
        &app,
        &id,
        json!({ "movie": { "name": "Alien", "year": 1979 } }),
    )
    .await;

    app.finish_transcode("movies/Alien (1979)/alien.mkv").await;
    let files = &app.get_json("/movies").await[0]["media_files"];
    assert_eq!(files[0]["label"], "alien");
    assert_eq!(files[0]["default"], true, "alien.mkv came first");
    assert_eq!(files[1]["label"], "alien-remastered");
    assert_eq!(files[1]["default"], false);
}

#[tokio::test]
async fn an_unassigned_media_file_can_be_attached_to_an_episode() {
    let app = TestApp::start().await;
    ready(&app, "shows/Breaking Bad (2008)/Season 01/S01E01.mkv").await;
    ready(&app, "shows/Breaking Bad (2008)/Season 01/S02E02.mkv").await;
    let id = only_unassigned_id(&app).await;

    let (status, _) = attach(
        &app,
        &id,
        json!({ "episode": { "show": "Breaking Bad", "year": 2008, "season": 1, "number": 1 } }),
    )
    .await;

    assert_eq!(status, 200);
    let show = app.get_json("/shows").await[0]["id"].clone();
    let episodes = app
        .get_json(&format!("/shows/{show}/seasons/1/episodes"))
        .await;
    let labels: Vec<&str> = episodes[0]["media_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["S01E01", "S02E02"]);
    assert_eq!(episodes[0]["media_files"][1]["id"], id);
}

#[tokio::test]
async fn attaching_to_a_movie_or_episode_that_does_not_exist_is_refused() {
    let app = TestApp::start().await;
    ready(&app, "shows/Breaking Bad (2008)/Season 01/S01E01.mkv").await;
    app.finish_transcode("misc/clip.mp4").await;
    let id = only_unassigned_id(&app).await;

    let no_movie = json!({ "movie": { "name": "Dune", "year": 1984 } });
    let no_episode =
        json!({ "episode": { "show": "Breaking Bad", "year": 2008, "season": 1, "number": 9 } });
    assert_eq!(attach(&app, &id, no_movie).await.0, 404);
    assert_eq!(attach(&app, &id, no_episode).await.0, 404);

    assert_eq!(only_unassigned_id(&app).await, id);
}

#[tokio::test]
async fn only_unassigned_media_files_can_be_attached() {
    let app = TestApp::start().await;
    ready(&app, "movies/The Matrix (1999)/matrix.mkv").await;
    ready(&app, "movies/Dune (2021)/dune.mkv").await;
    let matrix = app.get_json("/movies").await[1]["media_files"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let dune = json!({ "movie": { "name": "Dune", "year": 2021 } });

    assert_eq!(attach(&app, &matrix, dune.clone()).await.0, 404);
    assert_eq!(attach(&app, "no-such-id", dune).await.0, 404);
    assert_eq!(
        app.get_json("/movies").await[0]["media_files"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn an_attached_media_file_stays_attached_when_uploaded_again() {
    let app = TestApp::start().await;
    ready(&app, "movies/Dune (2021)/dune.mkv").await;
    ready(&app, "movies/Dune/dune-extended.mkv").await;
    let id = only_unassigned_id(&app).await;
    attach(
        &app,
        &id,
        json!({ "movie": { "name": "Dune", "year": 2021 } }),
    )
    .await;

    ready(&app, "movies/Dune/dune-extended.mkv").await;

    assert_eq!(unassigned(&app).await, json!([]));
    let files = &app.get_json("/movies").await[0]["media_files"];
    assert_eq!(files[1]["id"], id);
    assert_eq!(files[1]["label"], "dune-extended");
}

#[tokio::test]
async fn an_unassigned_media_file_can_be_attached_to_a_new_movie_from_a_tmdb_id() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    ready(&app, "misc/matrix.mkv").await;
    let id = only_unassigned_id(&app).await;

    let (status, body) = attach(&app, &id, json!({ "new_movie": { "tmdb_id": 603 } })).await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body["label"], "matrix");
    assert_eq!(unassigned(&app).await, json!([]));
    let movies = app.get_json("/movies").await;
    assert_eq!(movies[0]["name"], "The Matrix");
    assert_eq!(movies[0]["year"], 1999);
    assert_eq!(movies[0]["tmdb_id"], 603);
    assert_eq!(movies[0]["description"], "About The Matrix.");
    assert_eq!(movies[0]["media_files"][0]["id"], id);
    // Its folder is the Movie's name and year: later uploads there join it.
    ready(&app, "movies/The Matrix (1999)/matrix-4k.mkv").await;
    let movies = app.get_json("/movies").await;
    assert_eq!(movies.as_array().unwrap().len(), 1, "{movies}");
    assert_eq!(movies[0]["media_files"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn an_unassigned_media_file_can_be_attached_to_an_episode_of_a_new_show_from_a_tmdb_id() {
    let app = TestApp::start().await;
    app.tmdb.add_show(show(1396, "Breaking Bad", 2008));
    app.tmdb.add_season(1396, 1, season("Season 1"));
    app.tmdb.add_episode(
        1396,
        1,
        2,
        episode(62086, "Cat's in the Bag...", "2008-01-27"),
    );
    ready(&app, "misc/bb-102.mkv").await;
    let id = only_unassigned_id(&app).await;

    let to = json!({ "new_episode": { "tmdb_id": 1396, "season": 1, "number": 2 } });
    let (status, body) = attach(&app, &id, to).await;

    assert_eq!(status, 200, "{body}");
    let shows = app.get_json("/shows").await;
    assert_eq!(shows[0]["name"], "Breaking Bad");
    assert_eq!(shows[0]["tmdb_id"], 1396);
    let show = &shows[0]["id"];
    let seasons = app.get_json(&format!("/shows/{show}/seasons")).await;
    assert_eq!(seasons[0]["name"], "Season 1");
    let episodes = app
        .get_json(&format!("/shows/{show}/seasons/1/episodes"))
        .await;
    assert_eq!(episodes[0]["number"], 2);
    assert_eq!(episodes[0]["name"], "Cat's in the Bag...");
    assert_eq!(episodes[0]["media_files"][0]["id"], id);
}

#[tokio::test]
async fn attaching_to_a_new_entry_tmdb_does_not_know_is_refused() {
    let app = TestApp::start().await;
    ready(&app, "misc/matrix.mkv").await;
    let id = only_unassigned_id(&app).await;

    let (status, _) = attach(&app, &id, json!({ "new_movie": { "tmdb_id": 603 } })).await;
    assert_eq!(status, 400);
    let (status, _) = attach(
        &app,
        "no-such-id",
        json!({ "new_movie": { "tmdb_id": 603 } }),
    )
    .await;
    assert_eq!(status, 404);
    app.tmdb.add_movie(movie(603, "The Matrix", 1999));
    app.tmdb.set_unreachable(true);
    let (status, _) = attach(&app, &id, json!({ "new_movie": { "tmdb_id": 603 } })).await;
    assert_eq!(status, 503);
    assert_eq!(only_unassigned_id(&app).await, id);
    assert_eq!(app.get_json("/movies").await, json!([]));
}
