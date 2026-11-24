//! Ratings: TMDB's user score and vote count of Movies, Shows and Episodes,
//! set with every Match and refresh, never locked, and kept current by the
//! rating sweep (ADR 0005).

mod common;

use std::time::Duration;

use asset_server::ports::tmdb::{MovieDetails, ShowDetails};
use axum::http::StatusCode;
use common::TestApp;
use common::tmdb::{episode, movie, rated, season, show};
use serde_json::{Value, json};

const MATRIX: &str = "movies/The Matrix (1999)/matrix.mkv";

/// The fake clock's start, as the API writes timestamps.
const START: &str = "2026-11-17T00:00:00Z";

/// Uploads `key`, lets its transcode finish and runs a TMDB pass.
async fn ready(app: &TestApp, key: &str) {
    app.finish_transcode(key).await;
    app.match_metadata().await;
}

/// The only listed Movie.
async fn only_movie(app: &TestApp) -> Value {
    let movies = app.get_json("/movies").await;
    assert_eq!(movies.as_array().unwrap().len(), 1, "{movies}");
    movies[0].clone()
}

#[tokio::test]
async fn a_matched_movie_shows_tmdbs_score_and_vote_count() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(MovieDetails {
        rating: rated(8.2, 26_000),
        ..movie(603, "The Matrix", 1999)
    });

    ready(&app, MATRIX).await;

    let listed = only_movie(&app).await;
    assert_eq!(listed["rating"], json!(8.2));
    assert_eq!(listed["votes"], json!(26_000));
    assert_eq!(listed["rating_updated_at"], json!(START));
}

#[tokio::test]
async fn an_unmatched_movie_has_no_rating() {
    let app = TestApp::start().await;

    ready(&app, MATRIX).await;

    let listed = only_movie(&app).await;
    assert_eq!(listed["rating"], Value::Null);
    assert_eq!(listed["votes"], Value::Null);
    assert_eq!(listed["rating_updated_at"], Value::Null);
    let fields = listed.as_object().unwrap();
    assert!(fields.contains_key("rating") && fields.contains_key("rating_updated_at"));
}

const S01E01: &str = "shows/Breaking Bad (2008)/Season 01/S01E01.mkv";
const S01E02: &str = "shows/Breaking Bad (2008)/Season 01/S01E02.mkv";
const S01E03: &str = "shows/Breaking Bad (2008)/Season 01/S01E03.mkv";

/// The only listed Show.
async fn only_show(app: &TestApp) -> Value {
    let shows = app.get_json("/shows").await;
    assert_eq!(shows.as_array().unwrap().len(), 1, "{shows}");
    shows[0].clone()
}

/// The listed Episodes of Season 1 of the only listed Show.
async fn season_1(app: &TestApp) -> Value {
    let id = &only_show(app).await["id"];
    app.get_json(&format!("/shows/{id}/seasons/1/episodes"))
        .await
}

/// TMDB knows Breaking Bad (rated 8.9), its Season 1 and Episodes 1 to 3,
/// rated 7.9, 8.1 and 8.3.
fn add_breaking_bad(app: &TestApp) {
    app.tmdb.add_show(ShowDetails {
        rating: rated(8.9, 15_000),
        ..show(1396, "Breaking Bad", 2008)
    });
    app.tmdb.add_season(1396, 1, season("Season 1"));
    for (number, id, name, score) in [
        (1, 62085, "Pilot", 7.9),
        (2, 62086, "Cat's in the Bag...", 8.1),
        (3, 62087, "...And the Bag's in the River", 8.3),
    ] {
        app.tmdb
            .add_episode(1396, 1, number, episode(id, name, "2008-01-20"));
        app.tmdb
            .rate_episode(1396, 1, number, rated(score, 100 + i64::from(number)));
    }
}

/// Episodes 1 to 3 of Breaking Bad, Ready and looked up.
async fn ready_breaking_bad(app: &TestApp) {
    for key in [S01E01, S01E02, S01E03] {
        app.finish_transcode(key).await;
    }
    app.reconcile().await;
    app.match_metadata().await;
}

/// (rating, votes) of each listed Episode of Season 1.
async fn episode_ratings(app: &TestApp) -> Vec<(Value, Value)> {
    season_1(app)
        .await
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["rating"].clone(), e["votes"].clone()))
        .collect()
}

#[tokio::test]
async fn a_matched_show_and_its_episodes_show_their_ratings() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);

    ready_breaking_bad(&app).await;

    let listed = only_show(&app).await;
    assert_eq!(listed["rating"], json!(8.9));
    assert_eq!(listed["votes"], json!(15_000));
    assert_eq!(listed["rating_updated_at"], json!(START));
    assert_eq!(
        episode_ratings(&app).await,
        [
            (json!(7.9), json!(101)),
            (json!(8.1), json!(102)),
            (json!(8.3), json!(103))
        ]
    );
    assert_eq!(season_1(&app).await[2]["rating_updated_at"], json!(START));
}

#[tokio::test]
async fn episodes_take_their_ratings_from_the_season_details_in_one_request() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);

    ready_breaking_bad(&app).await;

    // The search, the Show, its Season once, and each Episode's own details
    // (names, imdbIDs): no request for an Episode's Rating.
    assert_eq!(app.tmdb.calls(), 1 + 1 + 1 + 3);
    assert_eq!(app.tmdb.season_calls(), 1);
    assert_eq!(season_1(&app).await[0]["rating"], json!(7.9));
}

#[tokio::test]
async fn an_unmatched_show_and_its_episodes_have_no_rating() {
    let app = TestApp::start().await;

    ready_breaking_bad(&app).await;

    assert_eq!(only_show(&app).await["rating"], Value::Null);
    let episode = &season_1(&app).await[0];
    assert_eq!(episode["rating"], Value::Null);
    assert_eq!(episode["votes"], Value::Null);
    assert_eq!(episode["rating_updated_at"], Value::Null);
}

#[tokio::test]
async fn seasons_have_no_rating() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);

    ready_breaking_bad(&app).await;

    let id = &only_show(&app).await["id"];
    let season = &app.get_json(&format!("/shows/{id}/seasons")).await[0];
    assert!(season.get("rating").is_none(), "{season}");
}

#[tokio::test]
async fn a_refresh_updates_the_rating_of_a_movie_with_locked_fields() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(MovieDetails {
        rating: rated(8.2, 26_000),
        ..movie(603, "The Matrix", 1999)
    });
    ready(&app, MATRIX).await;
    let id = only_movie(&app).await["id"].clone();
    let (status, body) = app
        .send_json(
            "PATCH",
            &format!("/admin/movies/{id}"),
            json!({ "description": "Mine." }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    app.tmdb.add_movie(MovieDetails {
        rating: rated(8.3, 27_000),
        overview: Some("TMDB's new text.".into()),
        ..movie(603, "The Matrix", 1999)
    });
    app.advance(Duration::from_secs(60));

    let (status, body) = app
        .send_json("POST", &format!("/admin/movies/{id}/refresh"), json!({}))
        .await;

    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let listed = only_movie(&app).await;
    assert_eq!(listed["rating"], json!(8.3));
    assert_eq!(listed["votes"], json!(27_000));
    assert_eq!(listed["rating_updated_at"], json!("2026-11-17T00:01:00Z"));
    assert_eq!(listed["description"], json!("Mine."));
}

#[tokio::test]
async fn a_refresh_updates_the_ratings_of_a_show_and_episodes_with_locked_fields() {
    let app = TestApp::start().await;
    add_breaking_bad(&app);
    ready_breaking_bad(&app).await;
    let id = only_show(&app).await["id"].clone();
    for (uri, field) in [
        (format!("/admin/shows/{id}"), "description"),
        (format!("/admin/shows/{id}/seasons/1/episodes/1"), "name"),
    ] {
        let (status, body) = app
            .send_json("PATCH", &uri, json!({ field: "Mine." }))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    app.tmdb.add_show(ShowDetails {
        rating: rated(9.0, 16_000),
        ..show(1396, "Breaking Bad", 2008)
    });
    app.tmdb.rate_episode(1396, 1, 1, rated(8.0, 200));

    let (status, body) = app
        .send_json("POST", &format!("/admin/shows/{id}/refresh"), json!({}))
        .await;

    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let listed = only_show(&app).await;
    assert_eq!(listed["rating"], json!(9.0));
    assert_eq!(listed["description"], json!("Mine."));
    let pilot = &season_1(&app).await[0];
    assert_eq!(
        (&pilot["rating"], &pilot["votes"]),
        (&json!(8.0), &json!(200))
    );
    assert_eq!(pilot["name"], json!("Mine."));
}

#[tokio::test]
async fn the_rating_cannot_be_edited_or_locked() {
    let app = TestApp::start().await;
    app.tmdb.add_movie(MovieDetails {
        rating: rated(8.2, 26_000),
        ..movie(603, "The Matrix", 1999)
    });
    add_breaking_bad(&app);
    ready(&app, MATRIX).await;
    ready_breaking_bad(&app).await;
    let movie = only_movie(&app).await["id"].clone();
    let show = only_show(&app).await["id"].clone();
    let entries = [
        format!("/admin/movies/{movie}"),
        format!("/admin/shows/{show}"),
        format!("/admin/shows/{show}/seasons/1/episodes/1"),
    ];

    for entry in &entries {
        for edit in [
            json!({ "rating": 1.0 }),
            json!({ "votes": 1 }),
            json!({ "rating_updated_at": null }),
            json!({ "description": "Mine.", "rating": 1.0 }),
        ] {
            let (status, body) = app.send_json("PATCH", entry, edit.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{entry} {edit}: {body}");
            let message = body.to_string();
            assert!(message.contains("never locked"), "{entry} {edit}: {body}");
        }
        let (status, body) = app
            .send_json("DELETE", &format!("{entry}/locks/rating"), json!({}))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{entry}: {body}");
    }

    let listed = only_movie(&app).await;
    assert_eq!(listed["rating"], json!(8.2));
    assert_eq!(listed["description"], json!("About The Matrix."));
    assert_eq!(only_show(&app).await["rating"], json!(8.9));
    assert_eq!(season_1(&app).await[0]["rating"], json!(7.9));
}
