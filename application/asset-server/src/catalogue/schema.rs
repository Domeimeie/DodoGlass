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
        is_default -> Bool,
    }
}

diesel::joinable!(media_file -> movie (movie_id));
diesel::allow_tables_to_appear_in_same_query!(movie, media_file);
