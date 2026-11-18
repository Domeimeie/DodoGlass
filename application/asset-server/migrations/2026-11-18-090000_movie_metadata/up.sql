-- Movie metadata from TMDB (Match, Needs review, retries) and Locked fields.
-- Portable SQL: SQLite and Postgres both accept every statement (ADR 0002).
--
-- A Movie without a `movie_metadata` row has not been looked up yet. Its
-- displayed `name` and `year` stay in `movie`: a Match overwrites them there.

CREATE TABLE movie_metadata (
    movie_id INTEGER PRIMARY KEY NOT NULL REFERENCES movie (id) ON DELETE CASCADE,
    -- matched: filled from TMDB; needs_review: waiting for an admin to pick
    -- one of `movie_candidate`; retrying: TMDB was unavailable, next try at
    -- `retry_at_ms`.
    match_state VARCHAR(16) NOT NULL
        CHECK (match_state IN ('matched', 'needs_review', 'retrying')),
    -- Failed lookups in a row (TMDB unavailable), for the backoff.
    lookup_attempts INTEGER NOT NULL DEFAULT 0,
    retry_at_ms BIGINT,
    tmdb_id BIGINT,
    imdb_id VARCHAR(16),
    description TEXT,
    -- Key of the poster in the `assets` bucket, served at GET /images/<key>.
    image_key TEXT,
    -- Locked fields: a JSON array of the field names an admin edited, out of
    -- "name", "year", "imdb_id", "description", "image". A Match or a refresh
    -- never writes them.
    locked_fields TEXT NOT NULL DEFAULT '[]'
);

-- The top TMDB search results of a Movie in Needs review, best first.
CREATE TABLE movie_candidate (
    movie_id INTEGER NOT NULL REFERENCES movie (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    tmdb_id BIGINT NOT NULL,
    name TEXT NOT NULL,
    year INTEGER,
    description TEXT,
    -- TMDB's image path (`/abc.jpg`): candidates' posters are not downloaded.
    poster_path TEXT,
    PRIMARY KEY (movie_id, position)
);
