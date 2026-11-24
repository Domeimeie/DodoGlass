# DodoGlass Asset server

The catalogue of Movies, Shows (Seasons, Episodes) and their Media files. It learns of a Media file
once its transcode is complete: RustFS announces each new output marker (`<id>/source.json`) in
`final` on `dodoglass/final` (ADR 0006), and the asset server places it as Ready from the upload
path the marker records (ADR 0003). A periodic reconcile against `final` catches lost events and
gone outputs (ADR 0001). New Movies and Shows are matched against
TMDB (a Show's Seasons and Episodes by number), and their images stored in `assets`. Their
Ratings (TMDB's user score) are kept current by a periodic sweep (ADR 0005). The HTTP API serves
the catalogue and the images.

Design: [Asset server architecture page](../../docs/architecture/asset-server.md). Terms:
`GLOSSARY.md` at the repository root.

## Running it

Requirements: Rust (stable, edition 2024) and [go-task](https://taskfile.dev). SQLite is built in.

From the repository root:

```bash
go-task infra:up                    # RustFS + Mosquitto, buckets ingest, final and assets
go-task transcoder:serve-s3         # in a second terminal
go-task asset-server:serve          # in a third terminal; HTTP API on 127.0.0.1:8090
go-task infra:upload KEY="movies/The Matrix (1999)/matrix.mkv" -- application/transcoder/sample.mp4
curl -s localhost:8090/movies       # listed as soon as the transcode is complete (its marker's
                                    # event; or the next reconcile: every 300 s, --reconcile-interval)
```

TMDB lookups need an API key (v3 key or v4 read access token). Put it into a `.env` file in this
folder, which git ignores and `go-task asset-server:serve` loads: `ASSETS_TMDB_API_KEY=...`. A value
exported in the shell takes precedence. Without one, Movies and Shows keep the name and year from their folder.

`go-task asset-server:serve -- --help` lists every setting. Each one can also be set with an
`ASSETS_*` environment variable. The database is `assets.db` in this folder by default; migrations
run at startup.

| Flag | Environment variable | Default | |
|---|---|---|---|
| `--mqtt-url` | `ASSETS_MQTT_URL` | `mqtt://localhost:1883` | broker the marker events arrive through |
| `--mqtt-topic` | `ASSETS_MQTT_TOPIC` | `dodoglass/final` | topic of the new output markers in `final` |
| `--mqtt-group` | `ASSETS_MQTT_GROUP` | `assets` | shared-subscription group |
| `--final-bucket`, `--assets-bucket` | `ASSETS_FINAL_BUCKET`, `ASSETS_ASSETS_BUCKET` | `final`, `assets` | the buckets it reads and writes; it does not use `ingest` |
| `--reconcile-interval` | `ASSETS_RECONCILE_INTERVAL` | `300` | seconds between two reconciles |

The ingest topic, the status topic and the ingest bucket are no longer settings: the asset server
does not read them (ADR 0006).

With a TMDB key, a rating sweep fetches the Ratings that have become too old again:

| Flag | Environment variable | Default | |
|---|---|---|---|
| `--rating-max-age` | `ASSETS_RATING_MAX_AGE` | `7d` | Ratings older than this are fetched again |
| `--rating-sweep-interval` | `ASSETS_RATING_SWEEP_INTERVAL` | `1h` | time between two sweeps |

Both take a duration such as `7d`, `36h` or `1h 30m`. A sweep fetches at most 20 entries, oldest
first.

Other tasks (from this folder): `go-task test`, `go-task lint`, `go-task fmt`, `go-task check`
(fmt, clippy, tests).

## Code layout

| Module | What it does |
|---|---|
| `catalogue/` | The core: placing finished uploads (`placement.rs`), Media file states (Ready, Missing), Labels and the Default media file per owner (`media_files.rs`), Movies and what viewers see, including which Media file is shown as default (`movies.rs`), Shows, Seasons and Episodes (`shows.rs`), Unassigned media files and attaching them (`unassigned.rs`), TMDB outcomes (`match_state.rs`: the `MatchState` of an entry) and Locked fields of Movies (`metadata.rs`) and of Shows, Seasons and Episodes (`show_metadata.rs`), Ratings and what the rating sweep reads and writes (`ratings.rs`), fields edited by hand (locking them), unlocking, and what a picked Match or refresh applies to (`corrections.rs`), the admin's changes: default, Labels, removing Media files, deleting Hidden entries (`admin.rs`). Everything else goes through it |
| `ports/` | Marker events (`marker_events.rs`, MQTT in `mqtt_markers.rs` on top of `mqtt_client.rs`), buckets (`storage.rs`), the clock (`clock.rs`), TMDB (`tmdb.rs`: the `Tmdb` trait and the reqwest client) |
| `markers/` | The marker listener: S3 events for `<id>/source.json` in `final` → `Reconciler::marker_seen` (that one Media file), ack after storing |
| `reconcile.rs` | Output markers in `final` → Ready, Missing; unseen outputs placed from their marker. One Media file for the marker listener, all of them at startup and periodically |
| `matcher/` | TMDB lookups for new Movies and Shows (strict Match, Needs review, retries with backoff), Seasons and Episodes of matched Shows by number, image download (`mod.rs`); the lookups an admin asks for: picking a Match, refresh, attaching to a new Movie or Show from a TMDB id (`admin.rs`); the rating sweep (`sweep.rs`) |
| `http/` | axum routes, one module per part of the API (`movies.rs`, `shows.rs`, `images.rs`), merged in `mod.rs`. Handlers take `State<Catalogue>`, or `State<Matcher>` for what needs TMDB (both from `ApiState`). The admin API is `http/admin/` (`media_files.rs`, `hidden.rs`, `review.rs`, `unassigned.rs`, `corrections.rs`: Match, refresh, edit and unlock), nested under `/admin`: add a module there and merge its `routes()` (paths relative to `/admin`) in `admin/mod.rs` |
| `app.rs` | Builds everything; used by `main.rs` and by the tests |
| `db.rs`, `migrations/` | SQLite connection, portable-SQL migrations (ADR 0002) |

## Writing a scenario test

Tests drive the whole asset server in-process through its HTTP API, using `tests/common/mod.rs`.
They describe inputs (finished transcodes and their marker events, outputs disappearing, time
passing) and assert only on HTTP responses: never on database rows or internal structs. Nothing runs on a timer.

```rust
mod common;
use common::TestApp;
use serde_json::json;

#[tokio::test]
async fn a_movie_upload_is_listed_once_its_transcode_is_ready() {
    let app = TestApp::start().await;              // empty catalogue, empty buckets

    // Output + source.json in final, and RustFS's event for the marker → marker listener.
    app.finish_transcode("movies/The Matrix (1999)/matrix.mkv").await;

    assert_eq!(app.get_json("/movies").await[0]["name"], json!("The Matrix"));
}
```

What `TestApp` offers:

- **Inputs:** `finish_transcode(key)` (the output and `<id>/source.json` for upload `key` appear in
  `final`, and the marker's event goes through the marker listener),
  `finish_transcode_without_event(key)` (the same, but the event is lost: only a reconcile finds
  it), `start_retranscode(key)` (a re-upload: the transcoder deletes the old marker, no event),
  `lose_output(key)` (output deleted, then a reconcile: Missing), `send_marker_event(payload)` with
  `common::final_event(&[keys])` (RustFS's event for those objects in `final`, form-encoded, several
  records; e.g. the same marker twice, or other files), `put(&app.buckets.final_, key, bytes)` for
  anything else in a bucket (`buckets.final_`, `buckets.assets` are `object_store` in memory),
  `delete_prefix(&bucket, prefix)`.
- **TMDB:** `app.tmdb` is a `FakeTmdb` (`tests/common/tmdb.rs`), empty by default (every search
  finds nothing). `add_movie(common::tmdb::movie(603, "The Matrix", 1999))` (with a Rating:
  `MovieDetails { rating: tmdb::rated(8.2, 26_000), ..tmdb::movie(...) }`, the same for Shows),
  `add_show(tmdb::show(1396, "Breaking Bad", 2008))`, `add_season(1396, 1, tmdb::season("Season
  1"))`, `add_episode(1396, 1, 2, tmdb::episode(62086, "Cat's in the Bag...", "2008-01-27"))`
  (Seasons and Episodes it lacks answer "not found"; adding one again with the same id replaces
  it: "TMDB's entry changed"), `rate_episode(1396, 1, 2, tmdb::rated(8.1, 900))` (an Episode's
  Rating, as its Season's details list it), `add_image(path, bytes)`,
  `set_unreachable(true)`, `calls()`, `searches()`, `season_calls()`.
- **Triggers:** `reconcile()`, `match_metadata()` (one pass of TMDB lookups and due retries),
  `sweep_ratings()` (one rating sweep: the Ratings older than the maximum age, oldest first, up to
  a batch), `restart()` (same database and buckets, so the startup reconcile runs:
  "the asset server was down"), `advance(duration)` to move the fake clock (e.g. past the 7-day
  maximum age before a sweep).
- **Settings:** `TestApp::start_with(|config| config.rating_sweep_batch = 2)` changes the
  `app::Config` (also `rating_max_age`); `start()` uses the defaults.
- **HTTP:** `get(uri)` → `(status, json)`, `get_json(uri)` (asserts 200), `get_bytes(uri)` →
  `(status, content type, body)`, `send_json(method, uri,
  body)`, `request(request)`.

`finish_transcode` and `send_marker_event` return once the marker listener has acknowledged the
message, so the next line can rely on it being stored. Each `TestApp` has its own in-memory SQLite database with the real
migrations, so tests run in parallel. A new port's fake goes into `tests/common/` (like `tmdb.rs`)
and is passed to `App::start` next to the buckets, the clock and TMDB.

Put scenarios in a file per topic (`tests/movies.rs`, `tests/media_files.rs`, `tests/shows.rs`, ...), each starting with
`mod common;`.
