-- Several Media files per Movie (or Episode): unique Labels and one Default
-- media file per owner. Portable SQL (ADR 0002).
--
-- The default is a flag on the Media file, not a column on its owner, so that
-- Movies and Episodes share it: each owner kind gets its own unique indexes.

-- It runs after the shows migration, so `episode_id` exists.

ALTER TABLE media_file ADD COLUMN is_default BOOLEAN NOT NULL DEFAULT FALSE;

-- Media files stored before this migration: the oldest of each Movie is its
-- default...
UPDATE media_file SET is_default = TRUE
WHERE movie_id IS NOT NULL
  AND id = (
    SELECT first.id FROM media_file first
    WHERE first.movie_id = media_file.movie_id
    ORDER BY first.created_ms, first.id
    LIMIT 1
  );

-- ...and a Label already taken in the same Movie gets the id added.
UPDATE media_file SET label = label || ' (' || id || ')'
WHERE movie_id IS NOT NULL
  AND EXISTS (
    SELECT 1 FROM media_file earlier
    WHERE earlier.movie_id = media_file.movie_id
      AND earlier.label = media_file.label
      AND (earlier.created_ms < media_file.created_ms
           OR (earlier.created_ms = media_file.created_ms AND earlier.id < media_file.id))
  );

-- The same for the Media files of Episodes.
UPDATE media_file SET is_default = TRUE
WHERE episode_id IS NOT NULL
  AND id = (
    SELECT first.id FROM media_file first
    WHERE first.episode_id = media_file.episode_id
    ORDER BY first.created_ms, first.id
    LIMIT 1
  );

UPDATE media_file SET label = label || ' (' || id || ')'
WHERE episode_id IS NOT NULL
  AND EXISTS (
    SELECT 1 FROM media_file earlier
    WHERE earlier.episode_id = media_file.episode_id
      AND earlier.label = media_file.label
      AND (earlier.created_ms < media_file.created_ms
           OR (earlier.created_ms = media_file.created_ms AND earlier.id < media_file.id))
  );

-- NULL owners (Unassigned media files) never collide.
CREATE UNIQUE INDEX media_file_movie_label ON media_file (movie_id, label);
CREATE UNIQUE INDEX media_file_movie_default ON media_file (movie_id) WHERE is_default;
CREATE UNIQUE INDEX media_file_episode_label ON media_file (episode_id, label);
CREATE UNIQUE INDEX media_file_episode_default ON media_file (episode_id) WHERE is_default;
