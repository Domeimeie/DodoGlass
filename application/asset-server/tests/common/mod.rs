//! The test harness: the whole asset server in-process, driven through its
//! HTTP API. See "Writing a scenario test" in the README.
//!
//! The ports are replaced by in-memory versions:
//! - marker events (RustFS's events for new output markers in `final`) come
//!   from [`InMemoryMarkerEvents`], fed by [`TestApp::finish_transcode`];
//! - the `final` and `assets` buckets are `object_store` in memory;
//! - the clock is a [`FakeClock`] the test moves forward;
//! - TMDB is a [`FakeTmdb`] (`tmdb.rs`), empty until the test adds Movies;
//! - SQLite runs in memory, with the real migrations.
//!
//! Nothing runs on a timer: reconcile happens when the test calls
//! [`TestApp::reconcile`] (or at [`TestApp::restart`], like at a real startup),
//! TMDB lookups (and their retries) when it calls [`TestApp::match_metadata`],
//! a rating sweep when it calls [`TestApp::sweep_ratings`].

#![allow(dead_code)] // each test file uses a different part of the harness

pub mod tmdb;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use asset_server::app::{App, Config};
use asset_server::ports::clock::FakeClock;
use asset_server::ports::marker_events::{InMemoryMarkerEvents, MarkerEventSender};
use asset_server::ports::storage::Buckets;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tower::ServiceExt;

use tmdb::FakeTmdb;

/// A real event RustFS published (for an upload of `sample.mp4` to `ingest`).
const RUSTFS_EVENT: &str = include_str!("../../../transcoder/tests/fixtures/rustfs-event.json");

pub struct TestApp {
    app: App,
    router: Router,
    pub buckets: Buckets,
    pub clock: Arc<FakeClock>,
    pub tmdb: Arc<FakeTmdb>,
    config: Config,
    markers: MarkerEventSender,
    marker_listener: JoinHandle<()>,
}

impl TestApp {
    /// A fresh asset server with an empty catalogue and empty buckets.
    pub async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    /// As [`start`](Self::start), with settings changed by `configure`
    /// (e.g. `|config| config.rating_sweep_batch = 2`).
    pub async fn start_with(configure: impl FnOnce(&mut Config)) -> Self {
        static NEXT_DB: AtomicU64 = AtomicU64::new(0);
        // A named in-memory database, so that a restart can open it again.
        let database = format!(
            "file:asset-server-test-{}-{}?mode=memory&cache=shared",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        );
        let buckets = Buckets {
            final_: Arc::new(InMemory::new()),
            assets: Arc::new(InMemory::new()),
        };
        let mut config = Config {
            database,
            ..Config::default()
        };
        configure(&mut config);
        Self::start_on(
            config,
            buckets,
            Arc::new(FakeClock::default()),
            Arc::new(FakeTmdb::default()),
        )
        .await
    }

    async fn start_on(
        config: Config,
        buckets: Buckets,
        clock: Arc<FakeClock>,
        tmdb: Arc<FakeTmdb>,
    ) -> Self {
        let app = App::start(config.clone(), buckets.clone(), clock.clone(), tmdb.clone())
            .await
            .expect("the asset server starts");
        let (markers, source) = InMemoryMarkerEvents::new();
        let marker_listener = app.spawn_marker_listener(source);
        TestApp {
            markers,
            marker_listener,
            router: app.router(),
            app,
            buckets,
            clock,
            tmdb,
            config,
        }
    }

    /// Stops the asset server and starts it again on the same database and
    /// buckets: anything that happened meanwhile is only seen by the startup
    /// reconcile.
    pub async fn restart(self) -> Self {
        let TestApp {
            app,
            buckets,
            clock,
            tmdb,
            config,
            marker_listener,
            ..
        } = self;
        marker_listener.abort();
        // Keep the in-memory database alive while the old app goes away.
        let keep = app.database();
        drop(app);
        let restarted = Self::start_on(config, buckets, clock, tmdb).await;
        drop(keep);
        restarted
    }

    // --- inputs -----------------------------------------------------------

    /// The transcoder finishes `key`: its output and output marker
    /// (`<id>/source.json`) appear in `final`, and RustFS's event for the
    /// marker goes through the marker listener. Returns once the listener has
    /// handled (acknowledged) it.
    pub async fn finish_transcode(&self, key: &str) {
        self.finish_transcode_without_event(key).await;
        let id = dodoglass_ids::task_id_from_key(key);
        self.send_marker_event(final_event(&[&format!("{id}/source.json")]))
            .await;
    }

    /// Any message on `dodoglass/final`; returns once the marker listener has
    /// handled (acknowledged) it.
    pub async fn send_marker_event(&self, payload: Vec<u8>) {
        self.markers.send(payload).await;
    }

    /// As [`finish_transcode`](Self::finish_transcode), but the event is lost
    /// (the asset server was down, the broker dropped it): only a reconcile
    /// finds the output.
    pub async fn finish_transcode_without_event(&self, key: &str) {
        let id = dodoglass_ids::task_id_from_key(key);
        for file in ["hls/master.m3u8", "dash/manifest.mpd"] {
            self.put(&self.buckets.final_, &format!("{id}/{file}"), b"manifest")
                .await;
        }
        let marker = json!({
            "input": { "bucket": "ingest", "key": key },
            "etag": "748965504cce42abd1c302a2971f98c7-2",
            "formats": ["hls", "dash"],
            "completed_ms": 1_793_128_170_038u64,
            "worker": "test-worker",
        });
        self.put(
            &self.buckets.final_,
            &format!("{id}/source.json"),
            &serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .await;
    }

    /// `key` is uploaded again and the transcoder starts on the new version:
    /// it deletes the old output marker first (no event for that), so the
    /// output is incomplete until [`finish_transcode`](Self::finish_transcode).
    pub async fn start_retranscode(&self, key: &str) {
        let id = dodoglass_ids::task_id_from_key(key);
        self.buckets
            .final_
            .delete(&Path::parse(format!("{id}/source.json")).unwrap())
            .await
            .unwrap();
    }

    /// The output of `key` disappears from `final` and a reconcile notices:
    /// its Media file is Missing (no Ready one left: its entry is Hidden).
    pub async fn lose_output(&self, key: &str) {
        let id = dodoglass_ids::task_id_from_key(key);
        self.delete_prefix(&self.buckets.final_, &format!("{id}/"))
            .await;
        self.reconcile().await;
    }

    /// Deletes everything under `prefix` (e.g. `<id>/`) in `bucket`.
    pub async fn delete_prefix(&self, bucket: &Arc<dyn ObjectStore>, prefix: &str) {
        let prefix = Path::parse(prefix).unwrap();
        let objects: Vec<_> = bucket.list(Some(&prefix)).collect().await;
        for object in objects {
            bucket.delete(&object.unwrap().location).await.unwrap();
        }
    }

    pub async fn put(&self, bucket: &Arc<dyn ObjectStore>, key: &str, content: &[u8]) {
        bucket
            .put(
                &Path::parse(key).unwrap(),
                PutPayload::from(content.to_vec()),
            )
            .await
            .unwrap();
    }

    /// One reconcile pass, as the periodic one in production.
    pub async fn reconcile(&self) {
        self.app.reconcile().await.expect("reconcile");
    }

    /// One pass of TMDB lookups: new Movies, and retries that are due.
    pub async fn match_metadata(&self) {
        self.app.match_metadata().await.expect("metadata pass");
    }

    /// One rating sweep: up to a batch of the Ratings older than the maximum
    /// age, oldest first (7 days and 20 by default; see [`start_with`](Self::start_with)).
    pub async fn sweep_ratings(&self) {
        self.app.sweep_ratings().await.expect("rating sweep");
    }

    pub fn advance(&self, by: Duration) {
        self.clock.advance(by);
    }

    // --- the HTTP API -----------------------------------------------------

    pub async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.request(Request::get(uri).body(Body::empty()).unwrap())
            .await
    }

    /// GET that must answer 200; returns the JSON body.
    pub async fn get_json(&self, uri: &str) -> Value {
        let (status, body) = self.get(uri).await;
        assert_eq!(status, StatusCode::OK, "GET {uri}: {body}");
        body
    }

    /// GET of anything that is not JSON (images): status, content type, body.
    pub async fn get_bytes(&self, uri: &str) -> (StatusCode, Option<String>, Vec<u8>) {
        let request = Request::get(uri).body(Body::empty()).unwrap();
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap().to_string());
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, content_type, bytes.to_vec())
    }

    pub async fn send_json(&self, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        self.request(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
    }

    pub async fn request(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()))
        };
        (status, body)
    }
}

/// RustFS's event for new objects `keys` in `final` (e.g. `<id>/source.json`),
/// one record per key, with the keys form-encoded the way S3 events carry
/// them. Built from a real event RustFS published (for an upload to `ingest`).
pub fn final_event(keys: &[&str]) -> Vec<u8> {
    let mut event: Value = serde_json::from_str(RUSTFS_EVENT).unwrap();
    let mut record = event["Records"][0].clone();
    record["bucket_name"] = json!("final");
    record["data"]["s3"]["bucket"]["name"] = json!("final");
    record["data"]["s3"]["bucket"]["arn"] = json!("arn:aws:s3:::final");
    event["Records"] = keys
        .iter()
        .map(|key| {
            let mut record = record.clone();
            record["object_name"] = json!(key);
            record["data"]["s3"]["object"]["key"] = json!(form_encode(key));
            record
        })
        .collect();
    serde_json::to_vec(&event).unwrap()
}

/// `The Matrix (1999)` → `The+Matrix+%281999%29`, as in S3 event keys.
fn form_encode(key: &str) -> String {
    const KEEP: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'/')
        .remove(b' ');
    utf8_percent_encode(key, KEEP).to_string().replace(' ', "+")
}
