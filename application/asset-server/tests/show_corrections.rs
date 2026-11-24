//! Admin metadata corrections of Shows, their Seasons and Episodes: picking
//! another Match refills the whole Show, Locked fields are kept at every
//! level.

mod common;

use asset_server::ports::tmdb::{EpisodeDetails, SeasonDetails};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use common::tmdb::{episode, season, show};
use serde_json::{Value, json};

const S01E01: &str = "shows/Breaking Bad (2008)/Season 01/S01E01.mkv";
const S01E02: &str = "shows/Breaking Bad (2008)/Season 01/S01E02.mkv";

/// Uploads `key`, lets its transcode finish and runs a TMDB pass.
async fn ready(app: &TestApp, key: &str) {
    app.finish_transcode(key).await;
    app.match_metadata().await;
}

/// The only listed Show.
async fn only_show(app: &TestApp) -> Value {
    let shows = app.get_json("/shows").await;
    assert_eq!(shows.as_array().unwrap().len(), 1, "{shows}");
    shows[0].clone()
}

async fn seasons(app: &TestApp) -> Value {
    let id = &only_show(app).await["id"];
    app.get_json(&format!("/shows/{id}/seasons")).await
}

async fn episodes(app: &TestApp, season: i32) -> Value {
    let id = &only_show(app).await["id"];
    app.get_json(&format!("/shows/{id}/seasons/{season}/episodes"))
        .await
}

/// TMDB knows Breaking Bad (Season 1, Episodes 1 and 2), and another Show
/// with a Season 1 and an Episode 1 only.
fn two_shows(app: &TestApp) {
    app.tmdb.add_show(show(1396, "Breaking Bad", 2008));
    app.tmdb.add_season(1396, 1, season("Season 1"));
    app.tmdb
        .add_episode(1396, 1, 1, episode(62085, "Pilot", "2008-01-20"));
    app.tmdb.add_episode(
        1396,
        1,
        2,
        episode(62086, "Cat's in the Bag...", "2008-01-27"),
    );
    app.tmdb.add_show(show(5000, "Metástasis", 2014));
    app.tmdb.add_season(5000, 1, season("Temporada 1"));
    app.tmdb
        .add_episode(5000, 1, 1, episode(90001, "Piloto", "2014-06-01"));
}

#[tokio::test]
async fn picking_another_match_for_a_show_refills_its_seasons_and_episodes() {
    let app = TestApp::start().await;
    two_shows(&app);
    ready(&app, S01E01).await;
    ready(&app, S01E02).await;
    assert_eq!(
        episodes(&app, 1).await[1]["name"],
        json!("Cat's in the Bag...")
    );
    let id = only_show(&app).await["id"].clone();
    let (status, body) = app
        .send_json(
            "PATCH",
            &format!("/admin/shows/{id}/seasons/1/episodes/1"),
            json!({ "description": "My own words." }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = app
        .send_json(
            "PUT",
            &format!("/admin/shows/{id}/match"),
            json!({ "tmdb_id": 5000 }),
        )
        .await;

    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let listed = only_show(&app).await;
    assert_eq!(listed["name"], json!("Metástasis"));
    assert_eq!(listed["tmdb_id"], json!(5000));
    assert_eq!(seasons(&app).await[0]["name"], json!("Temporada 1"));
    let episodes = episodes(&app, 1).await;
    assert_eq!(episodes[0]["name"], json!("Piloto"));
    assert_eq!(episodes[0]["tmdb_id"], json!(90001));
    assert_eq!(episodes[0]["description"], json!("My own words."));
    // The new Show has no Episode 2: it is known by its number only.
    assert_eq!(episodes[1]["name"], json!("Episode 2"));
    assert_eq!(episodes[1].get("description"), None);
    assert_eq!(episodes[1].get("tmdb_id"), None);
}

#[tokio::test]
async fn a_refresh_pulls_in_episode_changes_and_keeps_locked_fields_until_unlocked() {
    let app = TestApp::start().await;
    two_shows(&app);
    ready(&app, S01E01).await;
    let id = only_show(&app).await["id"].clone();
    let e01 = format!("/admin/shows/{id}/seasons/1/episodes/1");
    app.send_json("PATCH", &e01, json!({ "name": "The Pilot" }))
        .await;
    app.tmdb.add_episode(
        1396,
        1,
        1,
        EpisodeDetails {
            overview: Some("Walter White is diagnosed.".into()),
            ..episode(62085, "Pilot", "2008-01-20")
        },
    );

    let (status, body) = app
        .send_json("POST", &format!("/admin/shows/{id}/refresh"), json!({}))
        .await;

    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let listed = &episodes(&app, 1).await[0];
    assert_eq!(listed["description"], json!("Walter White is diagnosed."));
    assert_eq!(listed["name"], json!("The Pilot"));

    let (status, body) = app
        .request(
            Request::delete(format!("{e01}/locks/name"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["locked_fields"], json!([]));
    let (status, body) = app
        .send_json("POST", &format!("{e01}/refresh"), json!({}))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(episodes(&app, 1).await[0]["name"], json!("Pilot"));
}

#[tokio::test]
async fn a_season_is_edited_locked_and_refreshed_on_its_own() {
    let app = TestApp::start().await;
    two_shows(&app);
    ready(&app, S01E01).await;
    let id = only_show(&app).await["id"].clone();
    let s01 = format!("/admin/shows/{id}/seasons/1");
    let (status, body) = app
        .send_json("PATCH", &s01, json!({ "description": "The first one." }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    app.tmdb.add_season(
        1396,
        1,
        SeasonDetails {
            name: Some("Season One".into()),
            ..season("Season 1")
        },
    );

    let (status, body) = app
        .send_json("POST", &format!("{s01}/refresh"), json!({}))
        .await;

    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let listed = &seasons(&app).await[0];
    assert_eq!(listed["name"], json!("Season One"));
    assert_eq!(listed["description"], json!("The first one."));
}

#[tokio::test]
async fn a_season_or_episode_of_a_show_without_a_match_cannot_be_refreshed() {
    let app = TestApp::start().await;
    ready(&app, S01E01).await;
    let id = only_show(&app).await["id"].clone();

    let (status, _) = app
        .send_json(
            "POST",
            &format!("/admin/shows/{id}/seasons/1/episodes/1/refresh"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = app
        .send_json(
            "POST",
            &format!("/admin/shows/{id}/seasons/7/refresh"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_shows_edited_fields_are_kept_when_another_match_is_picked() {
    let app = TestApp::start().await;
    two_shows(&app);
    ready(&app, S01E01).await;
    let id = only_show(&app).await["id"].clone();
    let (status, body) = app
        .send_json(
            "PATCH",
            &format!("/admin/shows/{id}"),
            json!({ "name": "Breaking Bad (US)", "image": null }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["locked_fields"], json!(["image", "name"]));

    app.send_json(
        "PUT",
        &format!("/admin/shows/{id}/match"),
        json!({ "tmdb_id": 5000 }),
    )
    .await;

    let listed = only_show(&app).await;
    assert_eq!(listed["name"], json!("Breaking Bad (US)"));
    assert_eq!(listed["year"], json!(2014));
    assert_eq!(listed["description"], json!("About Metástasis."));
}
