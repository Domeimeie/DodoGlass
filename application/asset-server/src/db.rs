//! The SQLite database: one connection, migrations at startup (ADR 0002).
//!
//! SQLite allows one writer at a time and only one asset server runs, so a
//! single connection behind a mutex is enough. Diesel is synchronous, so every
//! use runs on tokio's blocking pool ([`Db::run`]).

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use diesel::connection::SimpleConnection;
use diesel::{Connection, SqliteConnection};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

#[derive(Clone)]
pub struct Db(Arc<Mutex<SqliteConnection>>);

impl Db {
    /// Opens (or creates) the database and runs pending migrations.
    ///
    /// `url` is a file path (`assets.db`) or an SQLite URI
    /// (`file:name?mode=memory&cache=shared` for tests).
    pub fn open(url: &str) -> Result<Db> {
        if !url.starts_with("file:")
            && let Some(dir) = std::path::Path::new(url).parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create the folder of {url}"))?;
        }
        let mut conn = SqliteConnection::establish(url)
            .with_context(|| format!("cannot open the database {url}"))?;
        conn.batch_execute(
            "PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000; PRAGMA journal_mode = WAL;",
        )?;
        conn.run_pending_migrations(MIGRATIONS)
            .map_err(|e| anyhow!("database migration failed: {e}"))?;
        Ok(Db(Arc::new(Mutex::new(conn))))
    }

    /// Runs `f` with the connection, on the blocking pool. Make `f` one
    /// transaction for anything that reads, decides and writes.
    pub async fn run<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut SqliteConnection) -> Result<T> + Send + 'static,
    {
        let db = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = db.lock().map_err(|_| anyhow!("database lock poisoned"))?;
            f(&mut conn)
        })
        .await?
    }
}
