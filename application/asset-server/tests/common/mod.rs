//! The test harness: the whole asset server in-process, driven through its
//! HTTP API. See "Writing a scenario test" in the README.
//!
//! The ports are replaced by in-memory versions:
//! - upload events come from [`InMemoryUploadEvents`], fed by [`TestApp::upload`];
//! - the `ingest`, `final` and `assets` buckets are `object_store` in memory;
//! - the clock is a [`FakeClock`] the test moves forward;
//! - TMDB is a [`FakeTmdb`] (`tmdb.rs`), empty until the test adds Movies;
//! - SQLite runs in memory, with the real migrations.
//!
//! Nothing runs on a timer: reconcile happens when the test calls
//! [`TestApp::reconcile`] (or at [`TestApp::restart`], like at a real startup),
//! TMDB lookups (and their retries) when it calls [`TestApp::match_metadata`].

#![allow(dead_code)] // each test file uses a different part of the harness

pub mod tmdb;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use asset_server::app::{App, Config};
use asset_server::ports::clock::FakeClock;
use asset_server::ports::storage::Buckets;
use asset_server::ports::upload_events::{InMemoryUploadEvents, UploadEventSender};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tower::ServiceExt;

use tmdb::FakeTmdb;

/// A real event RustFS published for an upload of `sample.mp4` to `ingest`.
const RUSTFS_EVENT: &str = include_str!("../../../transcoder/tests/fixtures/rustfs-event.json");

pub struct TestApp {
    app: App,
    router: Router,
    pub buckets: Buckets,
    pub clock: Arc<FakeClock>,
    pub tmdb: Arc<FakeTmdb>,
    database: String,
    events: UploadEventSender,
    listener: JoinHandle<()>,
}

impl TestApp {
    /// A fresh asset server with an empty catalogue and empty buckets.
    pub async fn start() -> Self {
        static NEXT_DB: AtomicU64 = AtomicU64::new(0);
        // A named in-memory database, so that a restart can open it again.
        let database = format!(
            "file:asset-server-test-{}-{}?mode=memory&cache=shared",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        );
        let buckets = Buckets {
            ingest: Arc::new(InMemory::new()),
            final_: Arc::new(InMemory::new()),
            assets: Arc::new(InMemory::new()),
        };
        Self::start_on(
            database,
            buckets,
            Arc::new(FakeClock::default()),
            Arc::new(FakeTmdb::default()),
        )
        .await
    }

    async fn start_on(
        database: String,
        buckets: Buckets,
        clock: Arc<FakeClock>,
        tmdb: Arc<FakeTmdb>,
    ) -> Self {
        let config = Config {
            database: database.clone(),
            ..Config::default()
        };
        let app = App::start(config, buckets.clone(), clock.clone(), tmdb.clone())
            .await
            .expect("the asset server starts");
        let (events, source) = InMemoryUploadEvents::new();
        let listener = app.spawn_ingest_listener(source);
        TestApp {
            router: app.router(),
            app,
            buckets,
            clock,
            tmdb,
            database,
            events,
            listener,
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
            database,
            listener,
            ..
        } = self;
        listener.abort();
        // Keep the in-memory database alive while the old app goes away.
        let keep = app.database();
        drop(app);
        let restarted = Self::start_on(database, buckets, clock, tmdb).await;
        drop(keep);
        restarted
    }

    // --- inputs -----------------------------------------------------------

    /// An upload of `key` to `ingest`: RustFS's event for it goes through the
    /// ingest listener. Returns once the listener has handled it.
    pub async fn upload(&self, key: &str) {
        self.send_event(s3_event(&[key])).await;
    }

    /// Any message on the ingest topic; returns once the listener has handled
    /// (acknowledged) it.
    pub async fn send_event(&self, payload: Vec<u8>) {
        self.events.send(payload).await;
    }

    /// The transcoder finishes `key`: its output and output marker
    /// (`<id>/source.json`) appear in `final`, as the transcoder writes them.
    pub async fn finish_transcode(&self, key: &str) {
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

/// The fixture exactly as RustFS sent it (an upload of `sample.mp4`).
pub fn rustfs_event() -> Vec<u8> {
    RUSTFS_EVENT.as_bytes().to_vec()
}

/// RustFS's event for uploads of `keys` to `ingest`, one record per key, with
/// the keys form-encoded the way S3 events carry them.
pub fn s3_event(keys: &[&str]) -> Vec<u8> {
    let mut event: Value = serde_json::from_str(RUSTFS_EVENT).unwrap();
    let record = event["Records"][0].clone();
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
