-- Ratings: TMDB's user score (0-10) and vote count of Movies, Shows and
-- Episodes, and when they were last fetched. Unlike the other fields they are
-- kept current by the rating sweep (ADR 0005) and are never Locked fields.
-- Seasons have no Rating. Portable SQL (ADR 0002).
--
-- All three NULL: no Rating (no Match, or TMDB has no votes yet when
-- `rating_updated_ms` is set). A matched entry with `rating_updated_ms` NULL
-- (matched before this migration) is the first the sweep refreshes.

ALTER TABLE movie_metadata ADD COLUMN rating DOUBLE PRECISION;
ALTER TABLE movie_metadata ADD COLUMN votes BIGINT;
-- When the Rating was last fetched (epoch ms).
ALTER TABLE movie_metadata ADD COLUMN rating_updated_ms BIGINT;

ALTER TABLE show_metadata ADD COLUMN rating DOUBLE PRECISION;
ALTER TABLE show_metadata ADD COLUMN votes BIGINT;
ALTER TABLE show_metadata ADD COLUMN rating_updated_ms BIGINT;

ALTER TABLE episode_metadata ADD COLUMN rating DOUBLE PRECISION;
ALTER TABLE episode_metadata ADD COLUMN votes BIGINT;
ALTER TABLE episode_metadata ADD COLUMN rating_updated_ms BIGINT;

-- The sweep looks for the oldest Ratings.
CREATE INDEX movie_metadata_rating_updated ON movie_metadata (rating_updated_ms);
CREATE INDEX show_metadata_rating_updated ON show_metadata (rating_updated_ms);
CREATE INDEX episode_metadata_rating_updated ON episode_metadata (rating_updated_ms);
