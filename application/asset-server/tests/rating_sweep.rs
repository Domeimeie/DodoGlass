//! The rating sweep: Ratings older than the maximum age (7 days by default)
//! are fetched again, oldest first, a batch at a time; nothing else of an
//! entry changes (ADR 0005). Tests move the fake clock and trigger a sweep
//! with `sweep_ratings()`.

mod common;

use std::time::Duration;

use asset_server::ports::tmdb::{MovieDetails, ShowDetails};
use axum::http::StatusCode;
use common::TestApp;
use common::tmdb::{episode, movie, rated, season, show};
use serde_json::{Value, json};

const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(24 * 3600);

const MATRIX: &str = "movies/The Matrix (1999)/matrix.mkv";
const ALIEN: &str = "movies/Alien (1979)/alien.mkv";
const HEAT: &str = "movies/Heat (1995)/heat.mkv";
const S01E01: &str = "shows/Breaking Bad (2008)/Season 01/S01E01.mkv";
const S01E02: &str = "shows/Breaking Bad (2008)/Season 01/S01E02.mkv";

/// Uploads `key`, lets its transcode finish and runs a TMDB pass.
async fn ready(app: &TestApp, key: &str) {
    app.finish_transcode(key).await;
    app.match_metadata().await;
}

/// TMDB knows `title` from `year` as `tmdb_id`, rated `score`.
fn add_movie(app: &TestApp, tmdb_id: i64, title: &str, year: i32, score: f64) {
    app.tmdb.add_movie(MovieDetails {
        rating: rated(score, 1000),
        ..movie(tmdb_id, title, year)
    });
}

/// The listed Movie called `name`.
async fn movie_named(app: &TestApp, name: &str) -> Value {
    let movies = app.get_json("/movies").await;
    movies
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == json!(name))
        .unwrap_or_else(|| panic!("{name} not in {movies}"))
        .clone()
}

/// `entry` without its Rating: what the sweep must leave alone.
fn without_rating(entry: &Value) -> Value {
    let mut entry = entry.clone();
    for field in ["rating", "votes", "rating_updated_at"] {
        entry.as_object_mut().unwrap().remove(field);
    }
    entry
}

#[tokio::test]
async fn a_sweep_refreshes_stale_ratings_and_leaves_fresh_ones_and_everything_else() {
    let app = TestApp::start().await;
    add_movie(&app, 603, "The Matrix", 1999, 8.2);
    add_movie(&app, 348, "Alien", 1979, 8.1);
    ready(&app, MATRIX).await;
    app.advance(6 * DAY);
    ready(&app, ALIEN).await;
    app.advance(2 * DAY);
    // TMDB's entries changed meanwhile: new scores, and new descriptions.
    app.tmdb.add_movie(MovieDetails {
        rating: rated(8.4, 1100),
        overview: Some("TMDB's new text.".into()),
        ..movie(603, "The Matrix", 1999)
    });
    add_movie(&app, 348, "Alien", 1979, 8.5);
    let matrix_before = movie_named(&app, "The Matrix").await;
    let alien_before = movie_named(&app, "Alien").await;

    app.sweep_ratings().await;

    // The Matrix's Rating is 8 days old: fetched again.
    let matrix = movie_named(&app, "The Matrix").await;
    assert_eq!(matrix["rating"], json!(8.4));
    assert_eq!(matrix["votes"], json!(1100));
    assert_eq!(matrix["rating_updated_at"], json!("2026-11-25T00:00:00Z"));
    assert_eq!(without_rating(&matrix), without_rating(&matrix_before));
    assert_eq!(matrix["description"], json!("About The Matrix."));
    // Alien's is 2 days old: untouched.
    assert_eq!(movie_named(&app, "Alien").await, alien_before);
}

#[tokio::test]
async fn a_sweep_refreshes_a_shows_episodes_from_one_season_request() {
    let app = TestApp::start().await;
    app.tmdb.add_show(ShowDetails {
        rating: rated(8.9, 15_000),
        ..show(1396, "Breaking Bad", 2008)
    });
    app.tmdb.add_season(1396, 1, season("Season 1"));
    app.tmdb
        .add_episode(1396, 1, 1, episode(62085, "Pilot", "2008-01-20"));
    app.tmdb.add_episode(
        1396,
        1,
        2,
        episode(62086, "Cat's in the Bag...", "2008-01-27"),
    );
    app.tmdb.rate_episode(1396, 1, 1, rated(7.9, 101));
    app.tmdb.rate_episode(1396, 1, 2, rated(8.1, 102));
    ready(&app, S01E01).await;
    ready(&app, S01E02).await;
    let show_id = app.get_json("/shows").await[0]["id"].clone();
    let episodes_uri = format!("/shows/{show_id}/seasons/1/episodes");
    let episodes_before = app.get_json(&episodes_uri).await;
    app.advance(8 * DAY);
    app.tmdb.add_show(ShowDetails {
        rating: rated(9.0, 16_000),
        ..show(1396, "Breaking Bad", 2008)
    });
    app.tmdb.rate_episode(1396, 1, 1, rated(8.0, 201));
    app.tmdb.rate_episode(1396, 1, 2, rated(8.2, 202));
    let calls = app.tmdb.calls();

    app.sweep_ratings().await;

    // The Show's details and the Season's: none per Episode.
    assert_eq!(app.tmdb.calls() - calls, 2);
    let listed = &app.get_json("/shows").await[0];
    assert_eq!(listed["rating"], json!(9.0));
    assert_eq!(listed["rating_updated_at"], json!("2026-11-25T00:00:00Z"));
    let episodes = app.get_json(&episodes_uri).await;
    for (i, (rating, votes)) in [(8.0, 201), (8.2, 202)].into_iter().enumerate() {
        assert_eq!(episodes[i]["rating"], json!(rating));
        assert_eq!(episodes[i]["votes"], json!(votes));
        assert_eq!(
            episodes[i]["rating_updated_at"],
            json!("2026-11-25T00:00:00Z")
        );
        assert_eq!(
            without_rating(&episodes[i]),
            without_rating(&episodes_before[i])
        );
    }
}

#[tokio::test]
async fn a_sweep_updates_the_rating_of_an_entry_with_locked_fields_and_keeps_them() {
    let app = TestApp::start().await;
    add_movie(&app, 603, "The Matrix", 1999, 8.2);
    ready(&app, MATRIX).await;
    let id = movie_named(&app, "The Matrix").await["id"].clone();
    let (status, body) = app
        .send_json(
            "PATCH",
            &format!("/admin/movies/{id}"),
            json!({ "name": "Matrix", "description": "Mine." }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    app.advance(8 * DAY);
    add_movie(&app, 603, "The Matrix", 1999, 8.4);

    app.sweep_ratings().await;

    let listed = movie_named(&app, "Matrix").await;
    assert_eq!(listed["rating"], json!(8.4));
    assert_eq!(listed["description"], json!("Mine."));
    // Unlocking a field that is not locked answers the Locked fields.
    let (status, body) = app
        .send_json(
            "DELETE",
            &format!("/admin/movies/{id}/locks/year"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["locked_fields"], json!(["description", "name"]));
}

#[tokio::test]
async fn a_sweep_stops_after_its_batch_and_takes_the_oldest_ratings_first() {
    let app = TestApp::start_with(|config| config.rating_sweep_batch = 2).await;
    add_movie(&app, 603, "The Matrix", 1999, 8.2);
    add_movie(&app, 348, "Alien", 1979, 8.1);
    add_movie(&app, 949, "Heat", 1995, 7.9);
    // Matched an hour apart, Heat first.
    ready(&app, HEAT).await;
    app.advance(HOUR);
    ready(&app, ALIEN).await;
    app.advance(HOUR);
    ready(&app, MATRIX).await;
    app.advance(8 * DAY);
    add_movie(&app, 603, "The Matrix", 1999, 9.2);
    add_movie(&app, 348, "Alien", 1979, 9.1);
    add_movie(&app, 949, "Heat", 1995, 8.9);

    app.sweep_ratings().await;

    assert_eq!(movie_named(&app, "Heat").await["rating"], json!(8.9));
    assert_eq!(movie_named(&app, "Alien").await["rating"], json!(9.1));
    assert_eq!(movie_named(&app, "The Matrix").await["rating"], json!(8.2));

    app.sweep_ratings().await;

    assert_eq!(movie_named(&app, "The Matrix").await["rating"], json!(9.2));
}

#[tokio::test]
async fn tmdb_unreachable_during_a_sweep_keeps_the_rating_and_retries_it() {
    let app = TestApp::start().await;
    add_movie(&app, 603, "The Matrix", 1999, 8.2);
    ready(&app, MATRIX).await;
    app.advance(8 * DAY);
    add_movie(&app, 603, "The Matrix", 1999, 8.4);
    app.tmdb.set_unreachable(true);

    app.sweep_ratings().await;

    let listed = movie_named(&app, "The Matrix").await;
    assert_eq!(listed["rating"], json!(8.2));
    assert_eq!(listed["rating_updated_at"], json!("2026-11-17T00:00:00Z"));
    assert_eq!(
        app.get_json("/admin/needs-review").await,
        json!({ "movies": [], "shows": [] })
    );

    app.advance(HOUR);
    app.tmdb.set_unreachable(false);
    app.sweep_ratings().await;

    let listed = movie_named(&app, "The Matrix").await;
    assert_eq!(listed["rating"], json!(8.4));
    assert_eq!(listed["rating_updated_at"], json!("2026-11-25T01:00:00Z"));
}

#[tokio::test]
async fn unmatched_entries_are_not_swept() {
    let app = TestApp::start().await;
    // Two candidates: the Movie is in Needs review, without a Match.
    add_movie(&app, 603, "The Matrix", 1999, 8.2);
    add_movie(&app, 9999, "The Matrix", 1999, 5.0);
    ready(&app, MATRIX).await;
    app.advance(8 * DAY);
    let calls = app.tmdb.calls();

    app.sweep_ratings().await;

    assert_eq!(app.tmdb.calls(), calls);
    assert_eq!(movie_named(&app, "The Matrix").await["rating"], Value::Null);
}
