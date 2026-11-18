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
    }
}

diesel::joinable!(media_file -> movie (movie_id));
diesel::allow_tables_to_appear_in_same_query!(movie, media_file);

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
