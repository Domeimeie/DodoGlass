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
