//! The Diesel view of the tables, kept in sync with `migrations/` by hand.

diesel::table! {
    movie (id) {
        id -> Integer,
        folder_name -> Text,
        folder_year -> Integer,
        name -> Text,
        year -> Integer,
    }
}

diesel::table! {
    media_file (id) {
        id -> Text,
        source_key -> Text,
        movie_id -> Nullable<Integer>,
        label -> Text,
        state -> Text,
        state_since_ms -> BigInt,
        created_ms -> BigInt,
        hls_manifest -> Nullable<Text>,
        dash_manifest -> Nullable<Text>,
        error -> Nullable<Text>,
        episode_id -> Nullable<Integer>,
        is_default -> Bool,
    }
}

diesel::joinable!(media_file -> movie (movie_id));
diesel::allow_tables_to_appear_in_same_query!(movie, media_file);

diesel::table! {
    show (id) {
        id -> Integer,
        folder_name -> Text,
        folder_year -> Integer,
        name -> Text,
        year -> Integer,
    }
}

diesel::table! {
    season (id) {
        id -> Integer,
        show_id -> Integer,
        number -> Integer,
    }
}

diesel::table! {
    episode (id) {
        id -> Integer,
        season_id -> Integer,
        number -> Integer,
    }
}

diesel::joinable!(season -> show (show_id));
diesel::joinable!(episode -> season (season_id));
diesel::joinable!(media_file -> episode (episode_id));
diesel::allow_tables_to_appear_in_same_query!(show, season, episode, media_file);

diesel::table! {
    movie_metadata (movie_id) {
        movie_id -> Integer,
        match_state -> Text,
        lookup_attempts -> Integer,
        retry_at_ms -> Nullable<BigInt>,
        tmdb_id -> Nullable<BigInt>,
        imdb_id -> Nullable<Text>,
        description -> Nullable<Text>,
        image_key -> Nullable<Text>,
        locked_fields -> Text,
        rating -> Nullable<Double>,
        votes -> Nullable<BigInt>,
        rating_updated_ms -> Nullable<BigInt>,
    }
}

diesel::table! {
    movie_candidate (movie_id, position) {
        movie_id -> Integer,
        position -> Integer,
        tmdb_id -> BigInt,
        name -> Text,
        year -> Nullable<Integer>,
        description -> Nullable<Text>,
        poster_path -> Nullable<Text>,
    }
}

diesel::joinable!(movie_metadata -> movie (movie_id));
diesel::joinable!(movie_candidate -> movie (movie_id));
diesel::allow_tables_to_appear_in_same_query!(movie, movie_metadata, movie_candidate);
diesel::allow_tables_to_appear_in_same_query!(media_file, movie_metadata);
diesel::allow_tables_to_appear_in_same_query!(media_file, movie_candidate);

diesel::table! {
    show_metadata (show_id) {
        show_id -> Integer,
        match_state -> Text,
        lookup_attempts -> Integer,
        retry_at_ms -> Nullable<BigInt>,
        tmdb_id -> Nullable<BigInt>,
        imdb_id -> Nullable<Text>,
        description -> Nullable<Text>,
        image_key -> Nullable<Text>,
        locked_fields -> Text,
        rating -> Nullable<Double>,
        votes -> Nullable<BigInt>,
        rating_updated_ms -> Nullable<BigInt>,
    }
}

diesel::table! {
    show_candidate (show_id, position) {
        show_id -> Integer,
        position -> Integer,
        tmdb_id -> BigInt,
        name -> Text,
        year -> Nullable<Integer>,
        description -> Nullable<Text>,
        poster_path -> Nullable<Text>,
    }
}

diesel::table! {
    season_metadata (season_id) {
        season_id -> Integer,
        match_state -> Text,
        lookup_attempts -> Integer,
        retry_at_ms -> Nullable<BigInt>,
        name -> Nullable<Text>,
        description -> Nullable<Text>,
        image_key -> Nullable<Text>,
        locked_fields -> Text,
    }
}

diesel::table! {
    episode_metadata (episode_id) {
        episode_id -> Integer,
        match_state -> Text,
        lookup_attempts -> Integer,
        retry_at_ms -> Nullable<BigInt>,
        tmdb_id -> Nullable<BigInt>,
        name -> Nullable<Text>,
        description -> Nullable<Text>,
        image_key -> Nullable<Text>,
        air_date -> Nullable<Text>,
        imdb_id -> Nullable<Text>,
        locked_fields -> Text,
        rating -> Nullable<Double>,
        votes -> Nullable<BigInt>,
        rating_updated_ms -> Nullable<BigInt>,
    }
}

diesel::joinable!(show_metadata -> show (show_id));
diesel::joinable!(show_candidate -> show (show_id));
diesel::joinable!(season_metadata -> season (season_id));
diesel::joinable!(episode_metadata -> episode (episode_id));
diesel::allow_tables_to_appear_in_same_query!(
    show,
    show_metadata,
    show_candidate,
    season_metadata,
    episode_metadata,
);
diesel::allow_tables_to_appear_in_same_query!(season, show_metadata);
diesel::allow_tables_to_appear_in_same_query!(season, show_candidate);
diesel::allow_tables_to_appear_in_same_query!(season, season_metadata);
diesel::allow_tables_to_appear_in_same_query!(season, episode_metadata);
diesel::allow_tables_to_appear_in_same_query!(episode, show_metadata);
diesel::allow_tables_to_appear_in_same_query!(episode, show_candidate);
diesel::allow_tables_to_appear_in_same_query!(episode, season_metadata);
diesel::allow_tables_to_appear_in_same_query!(episode, episode_metadata);
diesel::allow_tables_to_appear_in_same_query!(media_file, show_metadata);
diesel::allow_tables_to_appear_in_same_query!(media_file, season_metadata);
diesel::allow_tables_to_appear_in_same_query!(media_file, episode_metadata);
