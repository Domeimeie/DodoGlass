-- A Media file belongs to a Movie, to an Episode, or to neither (Unassigned):
-- never to both. Portable SQL (ADR 0002).
--
-- SQLite cannot add a CHECK to an existing table, so the table is rebuilt:
-- created anew, filled from the old one, the old one dropped and the new one
-- renamed. Postgres accepts the same statements. No table references
-- media_file, so dropping it breaks no foreign key.

CREATE TABLE media_file_new (
    -- The task id (shared crate, ADR 0004): also the output folder in `final`.
    id VARCHAR(255) PRIMARY KEY NOT NULL,
    -- The upload key in `ingest`, byte for byte as uploaded (S3 keys are
    -- byte-exact); only the id is derived from its NFC form.
    source_key TEXT NOT NULL UNIQUE,
    -- Both NULL while the Media file is Unassigned.
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
    error TEXT,
    episode_id INTEGER REFERENCES episode (id),
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    CHECK (movie_id IS NULL OR episode_id IS NULL)
);

INSERT INTO media_file_new (
    id, source_key, movie_id, label, state, state_since_ms, created_ms,
    hls_manifest, dash_manifest, error, episode_id, is_default
)
SELECT
    id, source_key, movie_id, label, state, state_since_ms, created_ms,
    hls_manifest, dash_manifest, error, episode_id, is_default
FROM media_file;

DROP TABLE media_file;

ALTER TABLE media_file_new RENAME TO media_file;

-- The indexes went with the old table.
CREATE INDEX media_file_movie ON media_file (movie_id);
CREATE INDEX media_file_episode ON media_file (episode_id);
-- NULL owners (Unassigned media files) never collide.
CREATE UNIQUE INDEX media_file_movie_label ON media_file (movie_id, label);
CREATE UNIQUE INDEX media_file_movie_default ON media_file (movie_id) WHERE is_default;
CREATE UNIQUE INDEX media_file_episode_label ON media_file (episode_id, label);
CREATE UNIQUE INDEX media_file_episode_default ON media_file (episode_id) WHERE is_default;
