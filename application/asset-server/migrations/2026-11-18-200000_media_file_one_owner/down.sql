-- The same table without the one-owner CHECK, rebuilt as in up.sql.

CREATE TABLE media_file_old (
    id VARCHAR(255) PRIMARY KEY NOT NULL,
    source_key TEXT NOT NULL UNIQUE,
    movie_id INTEGER REFERENCES movie (id),
    label TEXT NOT NULL,
    state VARCHAR(16) NOT NULL
        CHECK (state IN ('processing', 'ready', 'failed', 'missing')),
    state_since_ms BIGINT NOT NULL,
    created_ms BIGINT NOT NULL,
    hls_manifest TEXT,
    dash_manifest TEXT,
    error TEXT,
    episode_id INTEGER REFERENCES episode (id),
    is_default BOOLEAN NOT NULL DEFAULT FALSE
);

INSERT INTO media_file_old (
    id, source_key, movie_id, label, state, state_since_ms, created_ms,
    hls_manifest, dash_manifest, error, episode_id, is_default
)
SELECT
    id, source_key, movie_id, label, state, state_since_ms, created_ms,
    hls_manifest, dash_manifest, error, episode_id, is_default
FROM media_file;

DROP TABLE media_file;

ALTER TABLE media_file_old RENAME TO media_file;

CREATE INDEX media_file_movie ON media_file (movie_id);
CREATE INDEX media_file_episode ON media_file (episode_id);
CREATE UNIQUE INDEX media_file_movie_label ON media_file (movie_id, label);
CREATE UNIQUE INDEX media_file_movie_default ON media_file (movie_id) WHERE is_default;
CREATE UNIQUE INDEX media_file_episode_label ON media_file (episode_id, label);
CREATE UNIQUE INDEX media_file_episode_default ON media_file (episode_id) WHERE is_default;
