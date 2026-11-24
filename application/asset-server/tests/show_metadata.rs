//! Show metadata from TMDB, and the cascade to its Seasons and Episodes:
//! matched by number once the Show has a Match, never in Needs review.

mod common;

use std::time::Duration;

use asset_server::ports::tmdb::{EpisodeDetails, SeasonDetails, ShowDetails};
use axum::http::StatusCode;
use common::TestApp;
use common::tmdb::{episode, season, show};
use serde_json::{Value, json};

const S01E01: &str = "shows/Breaking Bad (2008)/Season 01/S01E01.mkv";
const S01E02: &str = "shows/Breaking Bad (2008)/Season 01/S01E02.mkv";

/// Uploads `key` and lets its transcode finish and be found (no TMDB pass).
async fn ready(app: &TestApp, key: &str) {
    app.finish_transcode(key).await;
}

/// An Episode upload that is Ready (so listed), with the TMDB lookups done.
async fn ready_episode(app: &TestApp, key: &str) {
    ready(app, key).await;
    app.match_metadata().await;
}

/// The only listed Show.
async fn only_show(app: &TestApp) -> Value {
    let shows = app.get_json("/shows").await;
    assert_eq!(shows.as_array().unwrap().len(), 1, "{shows}");
    shows[0].clone()
}

/// The listed Seasons of the only listed Show.
async fn seasons(app: &TestApp) -> Value {
    let id = &only_show(app).await["id"];
    app.get_json(&format!("/shows/{id}/seasons")).await
}

/// The listed Episodes of Season `season` of the only listed Show.
async fn episodes(app: &TestApp, season: i32) -> Value {
    let id = &only_show(app).await["id"];
    app.get_json(&format!("/shows/{id}/seasons/{season}/episodes"))
        .await
}

/// TMDB knows Breaking Bad, its Season 1 and Episodes 1 and 2.
fn add_breaking_bad(app: &TestApp) {
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
}

#[tokio::test]
async fn one_exact_result_for_name_and_first_air_year_is_matched() {
    let app = TestApp::start().await;
    app.tmdb.add_show(show(1396, "Breaking Bad", 2008));
    app.tmdb.add_show(show(1397, "Breaking Bad", 2023));

    ready_episode(&app, S01E02).await;

    let listed = only_show(&app).await;
    assert_eq!(listed["name"], json!("Breaking Bad"));
    assert_eq!(listed["year"], json!(2008));
    assert_eq!(listed["tmdb_id"], json!(1396));
    assert_eq!(listed["imdb_id"], json!("tt0001396"));
    assert_eq!(listed["description"], json!("About Breaking Bad."));
}

#[tokio::test]
async fn a_match_fills_in_the_existing_seasons_and_episodes() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);
    ready(&app, S01E01).await;
    ready(&app, S01E02).await;

    app.match_metadata().await;

    assert_eq!(
        seasons(&app).await,
        json!([{ "number": 1, "name": "Season 1", "description": "About Season 1." }])
    );
    let episodes = episodes(&app, 1).await;
    assert_eq!(episodes[0]["name"], json!("Pilot"));
    let e2 = &episodes[1];
    assert_eq!(e2["number"], json!(2));
    assert_eq!(e2["name"], json!("Cat's in the Bag..."));
    assert_eq!(e2["description"], json!("About Cat's in the Bag...."));
    assert_eq!(e2["air_date"], json!("2008-01-27"));
    assert_eq!(e2["tmdb_id"], json!(62086));
}

#[tokio::test]
async fn an_episodes_imdb_id_and_still_come_from_tmdb() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);
    app.tmdb.add_episode(
        1396,
        1,
        2,
        EpisodeDetails {
            imdb_id: Some("tt1054724".into()),
            still_path: Some("/still.jpg".into()),
            ..episode(62086, "Cat's in the Bag...", "2008-01-27")
        },
    );
    app.tmdb.add_image("/still.jpg", b"the still");

    ready_episode(&app, S01E02).await;

    let e2 = &episodes(&app, 1).await[0];
    assert_eq!(e2["imdb_id"], json!("tt1054724"));
    let key = e2["image"].as_str().expect("a still");
    let (status, _, body) = app.get_bytes(&format!("/images/{key}")).await;
    assert_eq!(
        (status, body.as_slice()),
        (StatusCode::OK, &b"the still"[..])
    );
}

#[tokio::test]
async fn a_show_without_a_certain_result_needs_review_and_its_episodes_keep_their_numbers() {
    let app = TestApp::start().await;
    app.tmdb.add_show(show(1396, "Breaking Bad", 2008));
    app.tmdb.add_show(show(1397, "Breaking Bad", 2008));
    app.tmdb.add_season(1396, 1, season("Season 1"));
    app.tmdb.add_episode(
        1396,
        1,
        2,
        episode(62086, "Cat's in the Bag...", "2008-01-27"),
    );

    ready_episode(&app, S01E02).await;

    let listed = only_show(&app).await;
    assert_eq!(listed["name"], json!("Breaking Bad"));
    assert_eq!(listed.get("tmdb_id"), None);
    let review = app.get_json("/admin/needs-review").await;
    assert_eq!(review["movies"], json!([]));
    assert_eq!(review["shows"].as_array().unwrap().len(), 1, "{review}");
    assert_eq!(review["shows"][0]["id"], listed["id"]);
    assert_eq!(review["shows"][0]["name"], json!("Breaking Bad"));
    assert_eq!(review["shows"][0]["year"], json!(2008));
    let candidates: Vec<_> = review["shows"][0]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["tmdb_id"].clone())
        .collect();
    assert_eq!(candidates, [json!(1396), json!(1397)]);
    assert_eq!(
        seasons(&app).await,
        json!([{ "number": 1, "name": "Season 1" }])
    );
    let e2 = &episodes(&app, 1).await[0];
    assert_eq!(e2["name"], json!("Episode 2"));
    assert_eq!(e2.get("imdb_id"), None);
}

#[tokio::test]
async fn a_show_from_another_first_air_year_needs_review() {
    let app = TestApp::start().await;
    app.tmdb.add_show(show(1396, "Breaking Bad", 2009));

    ready_episode(&app, S01E02).await;

    let review = app.get_json("/admin/needs-review").await;
    assert_eq!(
        review["shows"][0]["candidates"],
        json!([{
            "tmdb_id": 1396,
            "name": "Breaking Bad",
            "year": 2009,
            "description": "About Breaking Bad.",
        }])
    );
}

#[tokio::test]
async fn an_unknown_special_keeps_its_number_as_its_name_and_is_not_in_review() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);

    ready_episode(&app, "shows/Breaking Bad (2008)/Specials/S00E03.mkv").await;

    assert_eq!(only_show(&app).await["tmdb_id"], json!(1396));
    assert_eq!(
        seasons(&app).await,
        json!([{ "number": 0, "name": "Specials" }])
    );
    let special = &episodes(&app, 0).await[0];
    assert_eq!(special["name"], json!("Episode 3"));
    assert_eq!(special.get("tmdb_id"), None);
    assert_eq!(
        app.get_json("/admin/needs-review").await["shows"],
        json!([])
    );
}

#[tokio::test]
async fn an_unknown_episode_of_a_known_season_keeps_its_number_as_its_name() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);

    ready_episode(&app, "shows/Breaking Bad (2008)/Season 01/S01E09.mkv").await;

    assert_eq!(seasons(&app).await[0]["name"], json!("Season 1"));
    assert_eq!(episodes(&app, 1).await[0]["name"], json!("Episode 9"));
}

#[tokio::test]
async fn images_fall_back_from_episode_to_season_to_show() {
    let app = TestApp::start().await;
    app.tmdb.add_show(ShowDetails {
        poster_path: Some("/show.jpg".into()),
        ..show(1396, "Breaking Bad", 2008)
    });
    app.tmdb.add_image("/show.jpg", b"show poster");
    app.tmdb.add_season(
        1396,
        2,
        SeasonDetails {
            poster_path: Some("/season2.jpg".into()),
            ..season("Season 2")
        },
    );
    app.tmdb.add_image("/season2.jpg", b"season 2 poster");
    app.tmdb.add_season(1396, 1, season("Season 1"));
    app.tmdb.add_episode(
        1396,
        2,
        1,
        EpisodeDetails {
            still_path: Some("/s02e01.jpg".into()),
            ..episode(2, "Seven Thirty-Seven", "2009-03-08")
        },
    );
    app.tmdb.add_image("/s02e01.jpg", b"s02e01 still");
    app.tmdb
        .add_episode(1396, 2, 2, episode(3, "Grilled", "2009-03-15"));
    ready(&app, S01E01).await;
    ready(&app, "shows/Breaking Bad (2008)/Season 02/S02E01.mkv").await;
    ready(&app, "shows/Breaking Bad (2008)/Season 02/S02E02.mkv").await;

    app.match_metadata().await;

    let image = |v: &Value| v["image"].as_str().expect("an image").to_string();
    let bytes = async |key: String| app.get_bytes(&format!("/images/{key}")).await.2;
    let show_image = image(&only_show(&app).await);
    assert_eq!(bytes(show_image.clone()).await, b"show poster");

    let seasons = seasons(&app).await;
    assert_eq!(image(&seasons[0]), show_image, "Season 1 has no poster");
    let season2 = image(&seasons[1]);
    assert_eq!(bytes(season2.clone()).await, b"season 2 poster");

    let s1 = episodes(&app, 1).await;
    assert_eq!(image(&s1[0]), show_image, "via Season 1, which has none");
    let s2 = episodes(&app, 2).await;
    assert_eq!(bytes(image(&s2[0])).await, b"s02e01 still");
    assert_eq!(image(&s2[1]), season2, "S02E02 has no still");
}

#[tokio::test]
async fn without_any_image_on_tmdb_nothing_is_shown() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);

    ready_episode(&app, S01E02).await;

    assert_eq!(only_show(&app).await.get("image"), None);
    assert_eq!(seasons(&app).await[0].get("image"), None);
    assert_eq!(episodes(&app, 1).await[0].get("image"), None);
}

#[tokio::test]
async fn a_new_episode_under_a_matched_show_is_filled_in_without_searching_again() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);
    ready_episode(&app, S01E01).await;
    let searches = app.tmdb.searches();

    ready_episode(&app, S01E02).await;

    assert_eq!(app.tmdb.searches(), searches);
    assert_eq!(
        episodes(&app, 1).await[1]["name"],
        json!("Cat's in the Bag...")
    );
}

#[tokio::test]
async fn a_new_season_under_a_matched_show_is_filled_in_too() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);
    app.tmdb.add_season(1396, 2, season("Season 2"));
    app.tmdb
        .add_episode(1396, 2, 1, episode(2, "Seven Thirty-Seven", "2009-03-08"));
    ready_episode(&app, S01E01).await;
    let searches = app.tmdb.searches();

    ready_episode(&app, "shows/Breaking Bad (2008)/Season 2/S02E01.mkv").await;

    assert_eq!(app.tmdb.searches(), searches);
    assert_eq!(seasons(&app).await[1]["name"], json!("Season 2"));
    assert_eq!(
        episodes(&app, 2).await[0]["name"],
        json!("Seven Thirty-Seven")
    );
}

#[tokio::test]
async fn a_looked_up_episode_is_not_looked_up_again() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);
    ready_episode(&app, S01E02).await;
    let calls = app.tmdb.calls();

    ready_episode(&app, "shows/Breaking Bad (2008)/Season 01/S01E02.mp4").await;
    app.advance(Duration::from_secs(24 * 3600));
    app.match_metadata().await;

    assert_eq!(app.tmdb.calls(), calls);
}

#[tokio::test]
async fn while_tmdb_is_unreachable_the_show_lookup_is_retried_with_backoff() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);
    app.tmdb.set_unreachable(true);

    ready_episode(&app, S01E02).await;
    assert_eq!(only_show(&app).await.get("tmdb_id"), None);
    assert_eq!(episodes(&app, 1).await[0]["name"], json!("Episode 2"));
    assert_eq!(
        app.get_json("/admin/needs-review").await["shows"],
        json!([])
    );

    app.tmdb.set_unreachable(false);
    app.match_metadata().await;
    assert_eq!(
        only_show(&app).await.get("tmdb_id"),
        None,
        "the retry waits for its backoff"
    );

    app.advance(Duration::from_secs(30));
    app.match_metadata().await;
    assert_eq!(only_show(&app).await["tmdb_id"], json!(1396));
    assert_eq!(
        episodes(&app, 1).await[0]["name"],
        json!("Cat's in the Bag...")
    );
}

#[tokio::test]
async fn a_long_outage_keeps_the_show_retrying_hourly_and_out_of_review() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);
    app.tmdb.set_unreachable(true);

    ready_episode(&app, S01E02).await;
    for _ in 0..(12 * 6) {
        app.advance(Duration::from_secs(5 * 60));
        app.match_metadata().await;
    }
    assert_eq!(
        app.get_json("/admin/needs-review").await["shows"],
        json!([]),
        "an outage does not fill the review queue"
    );

    app.tmdb.set_unreachable(false);
    app.advance(Duration::from_secs(3600));
    app.match_metadata().await;
    assert_eq!(only_show(&app).await["tmdb_id"], json!(1396));
    assert_eq!(
        episodes(&app, 1).await[0]["name"],
        json!("Cat's in the Bag...")
    );
}

#[tokio::test]
async fn an_episode_whose_lookup_hits_an_outage_is_retried_then_filled_in() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);
    ready_episode(&app, S01E01).await;
    app.tmdb.set_unreachable(true);

    ready_episode(&app, S01E02).await;
    assert_eq!(episodes(&app, 1).await[1]["name"], json!("Episode 2"));

    app.tmdb.set_unreachable(false);
    app.advance(Duration::from_secs(30));
    app.match_metadata().await;
    assert_eq!(
        episodes(&app, 1).await[1]["name"],
        json!("Cat's in the Bag...")
    );
}

#[tokio::test]
async fn an_episode_keeps_its_number_through_a_long_outage_and_is_filled_in_after() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);
    ready_episode(&app, S01E01).await;
    app.tmdb.set_unreachable(true);

    ready_episode(&app, S01E02).await;
    for _ in 0..(12 * 6) {
        app.advance(Duration::from_secs(5 * 60));
        app.match_metadata().await;
    }
    assert_eq!(episodes(&app, 1).await[1]["name"], json!("Episode 2"));
    assert_eq!(
        app.get_json("/admin/needs-review").await["shows"],
        json!([])
    );

    app.tmdb.set_unreachable(false);
    app.advance(Duration::from_secs(3600));
    app.match_metadata().await;
    assert_eq!(
        episodes(&app, 1).await[1]["name"],
        json!("Cat's in the Bag...")
    );
}
