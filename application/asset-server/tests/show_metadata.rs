//! Show metadata from TMDB, and the cascade to its Seasons and Episodes:
//! matched by number once the Show has a Match, never in Needs review.

mod common;

use common::TestApp;
use common::tmdb::show;
use serde_json::{Value, json};

const S01E02: &str = "shows/Breaking Bad (2008)/Season 01/S01E02.mkv";

/// An Episode upload that is Ready (so listed), with the TMDB lookups done.
async fn ready_episode(app: &TestApp, key: &str) {
    app.upload(key).await;
    app.finish_transcode(key).await;
    app.reconcile().await;
    app.match_metadata().await;
}

/// The only listed Show.
async fn only_show(app: &TestApp) -> Value {
    let shows = app.get_json("/shows").await;
    assert_eq!(shows.as_array().unwrap().len(), 1, "{shows}");
    shows[0].clone()
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
