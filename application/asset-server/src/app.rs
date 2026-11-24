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
use crate::matcher::Matcher;
use crate::ports::clock::Clock;
use crate::ports::marker_events::MarkerEvents;
use crate::ports::storage::Buckets;
use crate::ports::tmdb::Tmdb;
use crate::reconcile::Reconciler;
use crate::{http, markers};

#[derive(Debug, Clone)]
pub struct Config {
    /// SQLite file path, or an SQLite URI.
    pub database: String,
    /// The name of the `final` bucket, the first part of manifest keys.
    pub final_bucket: String,
    /// Ratings older than this are fetched again by the rating sweep.
    pub rating_max_age: Duration,
    /// How many entries one rating sweep fetches at most (a Season's
    /// Episodes count as one).
    pub rating_sweep_batch: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            database: "assets.db".into(),
            final_bucket: "final".into(),
            rating_max_age: crate::matcher::RATING_MAX_AGE,
            rating_sweep_batch: crate::matcher::RATING_SWEEP_BATCH,
        }
    }
}

pub struct App {
    db: Db,
    catalogue: Catalogue,
    reconciler: Reconciler,
    matcher: Matcher,
    assets: Arc<dyn object_store::ObjectStore>,
    rating_max_age: Duration,
    rating_sweep_batch: usize,
}

impl App {
    /// Opens the database (running migrations), then reconciles once, so
    /// that transcodes finished while the asset server was down are found.
    pub async fn start(
        config: Config,
        buckets: Buckets,
        clock: Arc<dyn Clock>,
        tmdb: Arc<dyn Tmdb>,
    ) -> Result<App> {
        let database = config.database.clone();
        let db = tokio::task::spawn_blocking(move || Db::open(&database)).await??;
        let catalogue = Catalogue::new(db.clone(), clock, buckets.final_.clone());
        let reconciler = Reconciler::new(catalogue.clone(), buckets.final_, &config.final_bucket);
        let matcher = Matcher::new(catalogue.clone(), tmdb, buckets.assets.clone());
        let app = App {
            db,
            catalogue,
            reconciler,
            matcher,
            assets: buckets.assets,
            rating_max_age: config.rating_max_age,
            rating_sweep_batch: config.rating_sweep_batch,
        };
        app.reconcile().await?;
        Ok(app)
    }

    pub fn router(&self) -> Router {
        let state = http::ApiState {
            catalogue: self.catalogue.clone(),
            matcher: self.matcher.clone(),
        };
        http::router(state).merge(http::images::router(self.assets.clone()))
    }

    /// One pass of TMDB lookups: new Movies, Shows, Seasons and Episodes, and
    /// retries that are due.
    pub async fn match_metadata(&self) -> Result<()> {
        self.matcher.run_once().await
    }

    /// Runs a pass of TMDB lookups now and then every `interval`.
    pub fn spawn_periodic_matcher(&self, interval: Duration) -> JoinHandle<()> {
        let matcher = self.matcher.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(interval);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticks.tick().await;
                if let Err(e) = matcher.run_once().await {
                    error!(error = format!("{e:#}"), "metadata pass failed");
                }
            }
        })
    }

    /// One rating sweep: up to a batch of the Ratings older than the maximum
    /// age, oldest first.
    pub async fn sweep_ratings(&self) -> Result<()> {
        self.matcher
            .sweep_ratings(self.rating_max_age, self.rating_sweep_batch)
            .await
    }

    /// Runs a rating sweep now and then every `interval`.
    pub fn spawn_periodic_rating_sweep(&self, interval: Duration) -> JoinHandle<()> {
        let matcher = self.matcher.clone();
        let (max_age, batch) = (self.rating_max_age, self.rating_sweep_batch);
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(interval);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticks.tick().await;
                if let Err(e) = matcher.sweep_ratings(max_age, batch).await {
                    error!(error = format!("{e:#}"), "rating sweep failed");
                }
            }
        })
    }

    /// One reconcile pass.
    pub async fn reconcile(&self) -> Result<()> {
        self.reconciler.run_once().await
    }

    /// Handles marker events from `events` until it closes.
    pub fn spawn_marker_listener(&self, events: impl MarkerEvents) -> JoinHandle<()> {
        tokio::spawn(markers::run(events, self.reconciler.clone()))
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
