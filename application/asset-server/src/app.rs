//! Builds the asset server from its parts. `main` and the tests use the same
//! constructor; only the ports differ.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::Router;
use tokio::task::JoinHandle;
use tracing::error;

use crate::catalogue::Catalogue;
use crate::db::Db;
use crate::ports::clock::Clock;
use crate::ports::status_hints::StatusHints;
use crate::ports::storage::Buckets;
use crate::ports::upload_events::UploadEvents;
use crate::reconcile::Reconciler;
use crate::{http, ingest, status};

#[derive(Debug, Clone)]
pub struct Config {
    /// SQLite file path, or an SQLite URI.
    pub database: String,
    /// The name of the `final` bucket, the first part of manifest keys.
    pub final_bucket: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            database: "assets.db".into(),
            final_bucket: "final".into(),
        }
    }
}

pub struct App {
    db: Db,
    catalogue: Catalogue,
    reconciler: Reconciler,
}

impl App {
    /// Opens the database (running migrations), then reconciles once, so
    /// that transcodes finished while the asset server was down are found.
    pub async fn start(config: Config, buckets: Buckets, clock: Arc<dyn Clock>) -> Result<App> {
        let database = config.database.clone();
        let db = tokio::task::spawn_blocking(move || Db::open(&database)).await??;
        let catalogue = Catalogue::new(db.clone(), clock);
        let reconciler = Reconciler::new(
            catalogue.clone(),
            buckets.ingest,
            buckets.final_,
            &config.final_bucket,
        );
        let app = App {
            db,
            catalogue,
            reconciler,
        };
        app.reconcile().await?;
        Ok(app)
    }

    pub fn router(&self) -> Router {
        http::router(self.catalogue.clone())
    }

    /// One reconcile pass.
    pub async fn reconcile(&self) -> Result<()> {
        self.reconciler.run_once().await
    }

    /// Handles status hints from `hints` until it closes.
    pub fn spawn_status_listener(&self, hints: impl StatusHints) -> JoinHandle<()> {
        tokio::spawn(status::run(
            hints,
            self.catalogue.clone(),
            self.reconciler.clone(),
        ))
    }

    /// Handles upload events from `events` until it closes.
    pub fn spawn_ingest_listener(&self, events: impl UploadEvents) -> JoinHandle<()> {
        tokio::spawn(ingest::run(events, self.catalogue.clone()))
    }

    /// Reconciles every `interval` (the first pass after one interval:
    /// [`start`](App::start) already did one).
    pub fn spawn_periodic_reconcile(&self, interval: Duration) -> JoinHandle<()> {
        let reconciler = self.reconciler.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(interval);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticks.tick().await;
            loop {
                ticks.tick().await;
                if let Err(e) = reconciler.run_once().await {
                    error!(error = format!("{e:#}"), "reconcile failed");
                }
            }
        })
    }

    /// The database handle; the database stays open while one is held.
    pub fn database(&self) -> Db {
        self.db.clone()
    }
}
