-- Movies and their Media files.
-- Portable SQL: SQLite and Postgres both accept every statement (ADR 0002),
-- except that integer ids are `INTEGER PRIMARY KEY`, relying on SQLite's
-- auto-assigned rowid: the one known rewrite for the move to Postgres (ADR
-- 0002, Consequences).

CREATE TABLE movie (
    id INTEGER PRIMARY KEY NOT NULL,
    -- Name and year from the upload folder `movies/<Name> (<Year>)`: they
    -- identify the Movie for placement and never change.
    folder_name TEXT NOT NULL,
    folder_year INTEGER NOT NULL,
    -- What viewers see: the folder's name and year until metadata replaces them.
    name TEXT NOT NULL,
    year INTEGER NOT NULL,
    UNIQUE (folder_name, folder_year)
);

CREATE TABLE media_file (
    -- The task id (shared crate, ADR 0004): also the output folder in `final`.
    id VARCHAR(255) PRIMARY KEY NOT NULL,
    -- The upload key in `ingest`, in Unicode NFC.
    source_key TEXT NOT NULL UNIQUE,
    -- NULL while the Media file is Unassigned.
    movie_id INTEGER REFERENCES movie (id),
    label TEXT NOT NULL,
    state VARCHAR(16) NOT NULL
        CHECK (state IN ('processing', 'ready', 'failed', 'missing')),
    -- When the Media file entered its current state (epoch ms).
    state_since_ms BIGINT NOT NULL,
    created_ms BIGINT NOT NULL,
    -- Manifest keys (`<final bucket>/<id>/hls/master.m3u8`), set when Ready.
    hls_manifest TEXT,
    dash_manifest TEXT,
    error TEXT
);

CREATE INDEX media_file_movie ON media_file (movie_id);
