# DodoGlass Asset server

The catalogue of Movies, Shows (Seasons, Episodes) and their Media files. It listens for uploads to
the `ingest` bucket, places each one in the catalogue from its path (ADR 0003), and marks it Ready
once the transcoder's output marker appears in `final` (ADR 0001). The HTTP API serves the catalogue.

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
curl -s localhost:8090/movies       # listed once the transcoder's `completed` status arrives
                                    # (or after the next reconcile: every 300 s, --reconcile-interval)
```

`go-task asset-server:serve -- --help` lists every setting. Each one can also be set with an
`ASSETS_*` environment variable. The database is `assets.db` in this folder by default; migrations
run at startup.

Other tasks (from this folder): `go-task test`, `go-task lint`, `go-task fmt`, `go-task check`
(fmt, clippy, tests).

## Code layout

| Module | What it does |
|---|---|
| `catalogue/` | The core: placing uploads (`placement.rs`), Media file states, Labels and the Default media file per owner (`media_files.rs`), Movies and what viewers see (`movies.rs`), Shows, Seasons and Episodes (`shows.rs`), Unassigned media files and attaching them (`unassigned.rs`). Everything else goes through it |
| `ports/` | Upload events (`upload_events.rs`, MQTT in `mqtt.rs`), status hints (`status_hints.rs`, MQTT in `mqtt_status.rs`), buckets (`storage.rs`), the clock (`clock.rs`) |
| `ingest/` | The ingest listener: S3 events → `Catalogue::upload_seen`, ack after storing |
| `reconcile.rs` | Output markers in `final` → Ready, Missing, Failed after 24 h; unseen outputs placed from their marker |
| `status.rs` | The status listener: `completed` → early reconcile of that Media file, `failed` → Failed |
| `http/` | axum routes, one module per part of the API (`movies.rs`, `shows.rs`), merged in `mod.rs`. The admin API is `http/admin/` (`media_files.rs`, `unassigned.rs`), nested under `/admin`: add a module there and merge its `routes()` (paths relative to `/admin`) in `admin/mod.rs` |
| `app.rs` | Builds everything; used by `main.rs` and by the tests |
| `db.rs`, `migrations/` | SQLite connection, portable-SQL migrations (ADR 0002) |

## Writing a scenario test

Tests drive the whole asset server in-process through its HTTP API, using `tests/common/mod.rs`.
They describe inputs (upload events, outputs appearing in buckets, time passing) and assert only on
HTTP responses: never on database rows or internal structs. Nothing runs on a timer.

```rust
mod common;
use common::TestApp;
use serde_json::json;

#[tokio::test]
async fn a_movie_upload_is_listed_once_its_transcode_is_ready() {
    let app = TestApp::start().await;              // empty catalogue, empty buckets

    app.upload("movies/The Matrix (1999)/matrix.mkv").await;  // RustFS event → ingest listener
    app.finish_transcode("movies/The Matrix (1999)/matrix.mkv").await; // output + source.json in final
    app.reconcile().await;                         // one reconcile pass

    assert_eq!(app.get_json("/movies").await[0]["name"], json!("The Matrix"));
}
```

What `TestApp` offers:

- **Inputs:** `upload(key)` (the RustFS event fixture with that key, form-encoded),
  `send_event(payload)` with `common::s3_event(&[keys])` (several records) or
  `common::rustfs_event()` (the fixture as is), `finish_transcode(key)`, `put(&app.buckets.final_, key,
  bytes)` for anything else in a bucket (`buckets.ingest`, `buckets.final_`, `buckets.assets` are
  `object_store` in memory), `delete_prefix(&bucket, prefix)` to make output disappear.
- **Status hints:** `send_completed(key)`, `fail_transcode(key, error)`, or any
  `send_status(json)` (e.g. `common::status_message(key, event, error)`); each returns once the status
  listener has handled it.
- **Triggers:** `reconcile()`, `restart()` (same database and buckets, so the startup reconcile runs:
  "the asset server was down"), `advance(duration)` to move the fake clock.
- **HTTP:** `get(uri)` → `(status, json)`, `get_json(uri)` (asserts 200), `send_json(method, uri,
  body)`, `request(request)`.

`upload` and `send_event` return once the listener has acknowledged the message, so the next line can
rely on it being stored. Each `TestApp` has its own in-memory SQLite database with the real
migrations, so tests run in parallel. A new port (e.g. a TMDB fake) goes into `tests/common/mod.rs`
and is passed to `App::start` next to the buckets and the clock.

Put scenarios in a file per topic (`tests/movies.rs`, `tests/media_files.rs`, `tests/shows.rs`, ...), each starting with
`mod common;`.
