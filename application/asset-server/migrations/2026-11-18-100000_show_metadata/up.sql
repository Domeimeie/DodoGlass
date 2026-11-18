-- Show, Season and Episode metadata from TMDB, and their Locked fields.
-- Portable SQL: SQLite and Postgres both accept every statement (ADR 0002).
--
-- Shows follow Movies (`movie_metadata`, `movie_candidate`): no row = not
-- looked up yet; the displayed `name` and `year` stay in `show`.
--
-- Seasons and Episodes are looked up by number once their Show has a Match:
-- no row = not looked up yet. They are never in Needs review: one TMDB does
-- not know is `not_found` and shows its number as its name.
-- `locked_fields` is a JSON array of field names, as in `movie_metadata`.

CREATE TABLE show_metadata (
    show_id INTEGER PRIMARY KEY NOT NULL REFERENCES show (id) ON DELETE CASCADE,
    match_state VARCHAR(16) NOT NULL
        CHECK (match_state IN ('matched', 'needs_review', 'retrying')),
    lookup_attempts INTEGER NOT NULL DEFAULT 0,
    retry_at_ms BIGINT,
    tmdb_id BIGINT,
    imdb_id VARCHAR(16),
    description TEXT,
    -- Key of the poster in the `assets` bucket.
    image_key TEXT,
    -- Out of "name", "year", "imdb_id", "description", "image".
    locked_fields TEXT NOT NULL DEFAULT '[]'
);

-- The top TMDB search results of a Show in Needs review, best first.
CREATE TABLE show_candidate (
    show_id INTEGER NOT NULL REFERENCES show (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    tmdb_id BIGINT NOT NULL,
    name TEXT NOT NULL,
    year INTEGER,
    description TEXT,
    poster_path TEXT,
    PRIMARY KEY (show_id, position)
);

CREATE TABLE season_metadata (
    season_id INTEGER PRIMARY KEY NOT NULL REFERENCES season (id) ON DELETE CASCADE,
    -- matched: filled from TMDB; not_found: TMDB does not know it (or the
    -- retries ran out); retrying: TMDB was unavailable, next try at
    -- `retry_at_ms`.
    match_state VARCHAR(16) NOT NULL
        CHECK (match_state IN ('matched', 'not_found', 'retrying')),
    lookup_attempts INTEGER NOT NULL DEFAULT 0,
    retry_at_ms BIGINT,
    -- NULL: the number is the name.
    name TEXT,
    description TEXT,
    -- NULL: the Show's poster is shown.
    image_key TEXT,
    -- Out of "name", "description", "image".
    locked_fields TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE episode_metadata (
    episode_id INTEGER PRIMARY KEY NOT NULL REFERENCES episode (id) ON DELETE CASCADE,
    match_state VARCHAR(16) NOT NULL
        CHECK (match_state IN ('matched', 'not_found', 'retrying')),
    lookup_attempts INTEGER NOT NULL DEFAULT 0,
    retry_at_ms BIGINT,
    tmdb_id BIGINT,
    -- NULL: the number is the name.
    name TEXT,
    description TEXT,
    -- Key of the still in `assets`; NULL: the Season's image is shown.
    image_key TEXT,
    -- ISO date, e.g. 2008-01-27.
    air_date VARCHAR(10),
    imdb_id VARCHAR(16),
    -- Out of "name", "description", "image", "air_date", "imdb_id".
    locked_fields TEXT NOT NULL DEFAULT '[]'
);
