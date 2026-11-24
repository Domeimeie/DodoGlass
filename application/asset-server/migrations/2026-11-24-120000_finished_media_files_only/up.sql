-- The catalogue only holds finished output (ADR 0006): a Media file is Ready
-- or Missing. Processing and Failed ones go, and so does the error column
-- that only Failed ones had. Portable SQL (ADR 0002).
--
-- SQLite cannot change a CHECK, so the table is rebuilt as in
-- 2026-11-18-200000_media_file_one_owner. No table references media_file.

DELETE FROM media_file WHERE state NOT IN ('ready', 'missing');

CREATE TABLE media_file_new (
    -- The task id (shared crate, ADR 0004): also the output folder in `final`.
    id VARCHAR(255) PRIMARY KEY NOT NULL,
    -- The input key its output marker records, byte for byte as uploaded (S3
    -- keys are byte-exact); only the id is derived from its NFC form.
    source_key TEXT NOT NULL UNIQUE,
    -- Both NULL while the Media file is Unassigned.
    movie_id INTEGER REFERENCES movie (id),
    label TEXT NOT NULL,
    state VARCHAR(16) NOT NULL
        CHECK (state IN ('ready', 'missing')),
    -- When the Media file entered its current state (epoch ms).
    state_since_ms BIGINT NOT NULL,
    created_ms BIGINT NOT NULL,
    -- Manifest keys (`<final bucket>/<id>/hls/master.m3u8`).
    hls_manifest TEXT,
    dash_manifest TEXT,
    episode_id INTEGER REFERENCES episode (id),
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    CHECK (movie_id IS NULL OR episode_id IS NULL)
);

INSERT INTO media_file_new (
    id, source_key, movie_id, label, state, state_since_ms, created_ms,
    hls_manifest, dash_manifest, episode_id, is_default
)
SELECT
    id, source_key, movie_id, label, state, state_since_ms, created_ms,
    hls_manifest, dash_manifest, episode_id, is_default
FROM media_file;

DROP TABLE media_file;

ALTER TABLE media_file_new RENAME TO media_file;

CREATE INDEX media_file_movie ON media_file (movie_id);
CREATE INDEX media_file_episode ON media_file (episode_id);
CREATE UNIQUE INDEX media_file_movie_label ON media_file (movie_id, label);
CREATE UNIQUE INDEX media_file_movie_default ON media_file (movie_id) WHERE is_default;
CREATE UNIQUE INDEX media_file_episode_label ON media_file (episode_id, label);
CREATE UNIQUE INDEX media_file_episode_default ON media_file (episode_id) WHERE is_default;

-- A deleted Media file may have been its owner's default: the oldest
-- remaining one (Ready before Missing) takes over, as on removal.
UPDATE media_file SET is_default = TRUE
WHERE movie_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM media_file d WHERE d.movie_id = media_file.movie_id AND d.is_default
  )
  AND id = (
      SELECT o.id FROM media_file o
      WHERE o.movie_id = media_file.movie_id
      ORDER BY CASE o.state WHEN 'ready' THEN 0 ELSE 1 END, o.created_ms, o.id
      LIMIT 1
  );

UPDATE media_file SET is_default = TRUE
WHERE episode_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM media_file d WHERE d.episode_id = media_file.episode_id AND d.is_default
  )
  AND id = (
      SELECT o.id FROM media_file o
      WHERE o.episode_id = media_file.episode_id
      ORDER BY CASE o.state WHEN 'ready' THEN 0 ELSE 1 END, o.created_ms, o.id
      LIMIT 1
  );
