-- Shows, their Seasons and Episodes; Media files of Episodes.
-- Portable SQL: SQLite and Postgres both accept every statement (ADR 0002).

CREATE TABLE show (
    id INTEGER PRIMARY KEY NOT NULL,
    -- Name and first-air year from the upload folder `shows/<Name> (<Year>)`:
    -- they identify the Show for placement and never change.
    folder_name TEXT NOT NULL,
    folder_year INTEGER NOT NULL,
    -- What viewers see: the folder's name and year until metadata replaces them.
    name TEXT NOT NULL,
    year INTEGER NOT NULL,
    UNIQUE (folder_name, folder_year)
);

CREATE TABLE season (
    id INTEGER PRIMARY KEY NOT NULL,
    show_id INTEGER NOT NULL REFERENCES show (id),
    -- 0 holds the specials.
    number INTEGER NOT NULL,
    UNIQUE (show_id, number)
);

CREATE TABLE episode (
    id INTEGER PRIMARY KEY NOT NULL,
    season_id INTEGER NOT NULL REFERENCES season (id),
    number INTEGER NOT NULL,
    UNIQUE (season_id, number)
);

-- NULL for the Media files of Movies and while Unassigned.
ALTER TABLE media_file ADD COLUMN episode_id INTEGER REFERENCES episode (id);

CREATE INDEX media_file_episode ON media_file (episode_id);
