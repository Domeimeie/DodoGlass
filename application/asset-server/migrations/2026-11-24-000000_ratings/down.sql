DROP INDEX episode_metadata_rating_updated;
DROP INDEX show_metadata_rating_updated;
DROP INDEX movie_metadata_rating_updated;

ALTER TABLE episode_metadata DROP COLUMN rating_updated_ms;
ALTER TABLE episode_metadata DROP COLUMN votes;
ALTER TABLE episode_metadata DROP COLUMN rating;

ALTER TABLE show_metadata DROP COLUMN rating_updated_ms;
ALTER TABLE show_metadata DROP COLUMN votes;
ALTER TABLE show_metadata DROP COLUMN rating;

ALTER TABLE movie_metadata DROP COLUMN rating_updated_ms;
ALTER TABLE movie_metadata DROP COLUMN votes;
ALTER TABLE movie_metadata DROP COLUMN rating;
