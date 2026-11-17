//! The upload path parser, tested through `place_upload` only (ADR 0003).

use dodoglass_ids::{Placement, UnassignedReason, place_upload};

fn movie(name: &str, year: u16, label: &str) -> Placement {
    Placement::Movie {
        name: name.into(),
        year,
        label: label.into(),
    }
}

fn episode(show: &str, year: u16, season: u32, episode: u32, label: &str) -> Placement {
    Placement::Episode {
        show: show.into(),
        year,
        season,
        episode,
        label: label.into(),
    }
}

fn unassigned(reason: UnassignedReason) -> Placement {
    Placement::Unassigned(reason)
}

fn check(cases: &[(&str, Placement)]) {
    for (key, expected) in cases {
        assert_eq!(&place_upload(key), expected, "key {key:?}");
    }
}

#[test]
fn movies_are_placed_by_folder_and_labelled_by_file_name() {
    check(&[(
        "movies/The Matrix (1999)/matrix.mkv",
        movie("The Matrix", 1999, "matrix"),
    )]);
}

#[test]
fn episodes_are_placed_by_show_folder_season_folder_and_episode_code() {
    check(&[
        (
            "shows/Breaking Bad (2008)/Season 01/S01E02 - Cat's in the Bag.mkv",
            episode("Breaking Bad", 2008, 1, 2, "S01E02 - Cat's in the Bag"),
        ),
        (
            "shows/Breaking Bad (2008)/Season 1/S01E02.mkv",
            episode("Breaking Bad", 2008, 1, 2, "S01E02"),
        ),
        (
            "shows/Dark (2017)/Season 3/S03E08.mp4",
            episode("Dark", 2017, 3, 8, "S03E08"),
        ),
        (
            "shows/Breaking Bad (2008)/Specials/S00E01.mkv",
            episode("Breaking Bad", 2008, 0, 1, "S00E01"),
        ),
    ]);
}

#[test]
fn a_movie_or_show_folder_without_a_year_is_unassigned() {
    use UnassignedReason::NoYear;
    check(&[
        ("movies/The Matrix/matrix.mkv", unassigned(NoYear)),
        ("movies/The Matrix ()/matrix.mkv", unassigned(NoYear)),
        ("movies/The Matrix (19999)/matrix.mkv", unassigned(NoYear)),
        ("movies/The Matrix (abcd)/matrix.mkv", unassigned(NoYear)),
        ("movies/(1999)/matrix.mkv", unassigned(NoYear)),
        ("movies/The Matrix 1999/matrix.mkv", unassigned(NoYear)),
        (
            "movies/The Matrix (1999) extended/matrix.mkv",
            unassigned(NoYear),
        ),
        ("shows/Dark/Season 1/S01E01.mkv", unassigned(NoYear)),
    ]);
}

#[test]
fn an_upload_outside_movies_and_shows_is_unassigned() {
    use UnassignedReason::UnknownRootFolder;
    check(&[
        ("misc/clip.mp4", unassigned(UnknownRootFolder)),
        ("clip.mp4", unassigned(UnknownRootFolder)),
        ("Season 01/S02E01.mkv", unassigned(UnknownRootFolder)),
        (
            "Movies/The Matrix (1999)/matrix.mkv",
            unassigned(UnknownRootFolder),
        ),
        ("", unassigned(UnknownRootFolder)),
    ]);
}

#[test]
fn an_upload_at_the_wrong_depth_is_unassigned() {
    use UnassignedReason::WrongDepth;
    check(&[
        ("movies/matrix.mkv", unassigned(WrongDepth)),
        (
            "movies/The Matrix (1999)/extras/matrix.mkv",
            unassigned(WrongDepth),
        ),
        ("shows/Dark (2017)/S01E01.mkv", unassigned(WrongDepth)),
        (
            "shows/Dark (2017)/Season 1/extra/S01E01.mkv",
            unassigned(WrongDepth),
        ),
        ("movies/The Matrix (1999)/", unassigned(WrongDepth)),
    ]);
}

#[test]
fn the_episode_code_is_matched_in_any_case_at_the_start_of_the_file_name() {
    use UnassignedReason::NoEpisodeCode;
    check(&[
        (
            "shows/Dark (2017)/Season 1/s01e03.mkv",
            episode("Dark", 2017, 1, 3, "s01e03"),
        ),
        (
            "shows/Dark (2017)/Season 1/S1E3 Past and Present.mkv",
            episode("Dark", 2017, 1, 3, "S1E3 Past and Present"),
        ),
        (
            "shows/Dark (2017)/Season 1/S01E10.mkv",
            episode("Dark", 2017, 1, 10, "S01E10"),
        ),
        (
            "shows/Dark (2017)/Season 1/Dark S01E03.mkv",
            unassigned(NoEpisodeCode),
        ),
        (
            "shows/Dark (2017)/Season 1/Episode 3.mkv",
            unassigned(NoEpisodeCode),
        ),
        (
            "shows/Dark (2017)/Season 1/S01.mkv",
            unassigned(NoEpisodeCode),
        ),
        (
            "shows/Dark (2017)/Season 1/S01E.mkv",
            unassigned(NoEpisodeCode),
        ),
        (
            "shows/Dark (2017)/Season 1/SE03.mkv",
            unassigned(NoEpisodeCode),
        ),
    ]);
}

#[test]
fn an_episode_code_whose_season_differs_from_the_folder_is_unassigned() {
    use UnassignedReason::SeasonMismatch;
    check(&[
        (
            "shows/Dark (2017)/Season 01/S02E01.mkv",
            unassigned(SeasonMismatch),
        ),
        (
            "shows/Dark (2017)/Specials/S01E01.mkv",
            unassigned(SeasonMismatch),
        ),
        (
            "shows/Dark (2017)/Season 1/S00E01.mkv",
            unassigned(SeasonMismatch),
        ),
    ]);
}

#[test]
fn a_season_folder_other_than_season_n_or_specials_is_unassigned() {
    use UnassignedReason::UnknownSeasonFolder;
    check(&[
        (
            "shows/Dark (2017)/Extras/S01E01.mkv",
            unassigned(UnknownSeasonFolder),
        ),
        (
            "shows/Dark (2017)/Season/S01E01.mkv",
            unassigned(UnknownSeasonFolder),
        ),
        (
            "shows/Dark (2017)/Season one/S01E01.mkv",
            unassigned(UnknownSeasonFolder),
        ),
        (
            "shows/Dark (2017)/S1/S01E01.mkv",
            unassigned(UnknownSeasonFolder),
        ),
    ]);
}

#[test]
fn names_keep_spaces_parentheses_apostrophes_and_non_ascii_letters() {
    check(&[
        (
            "movies/Amélie (2001)/Amélie à Paris.mkv",
            movie("Amélie", 2001, "Amélie à Paris"),
        ),
        (
            "movies/Monty Python (and the Holy Grail) (1975)/grail (restored).mkv",
            movie(
                "Monty Python (and the Holy Grail)",
                1975,
                "grail (restored)",
            ),
        ),
        (
            "movies/Schindler's List (1993)/schindler's.mp4",
            movie("Schindler's List", 1993, "schindler's"),
        ),
        (
            "shows/Grey's Anatomy (2005)/Season 2/S02E01 Raindrops Keep Falling.mkv",
            episode(
                "Grey's Anatomy",
                2005,
                2,
                1,
                "S02E01 Raindrops Keep Falling",
            ),
        ),
        (
            "shows/Дом (2023)/Season 1/S01E01 Пилот.mkv",
            episode("Дом", 2023, 1, 1, "S01E01 Пилот"),
        ),
    ]);
}

/// The table under "Upload layout" in `docs/architecture/asset-server.md`.
#[test]
fn the_layout_examples_in_the_architecture_doc_parse_as_documented() {
    use UnassignedReason::*;
    check(&[
        (
            "movies/The Matrix (1999)/matrix.mkv",
            movie("The Matrix", 1999, "matrix"),
        ),
        (
            "movies/The Matrix (1999)/matrix-directors-cut.mkv",
            movie("The Matrix", 1999, "matrix-directors-cut"),
        ),
        (
            "shows/Breaking Bad (2008)/Season 01/S01E02 - Cat's in the Bag.mkv",
            episode("Breaking Bad", 2008, 1, 2, "S01E02 - Cat's in the Bag"),
        ),
        (
            "shows/Breaking Bad (2008)/Specials/S00E01.mkv",
            episode("Breaking Bad", 2008, 0, 1, "S00E01"),
        ),
        ("movies/The Matrix/matrix.mkv", unassigned(NoYear)),
        ("misc/clip.mp4", unassigned(UnknownRootFolder)),
        (
            "shows/Breaking Bad (2008)/Season 01/S02E01.mkv",
            unassigned(SeasonMismatch),
        ),
    ]);
}

#[test]
fn mp4_mov_and_mkv_are_accepted_in_any_case() {
    check(&[
        ("movies/Heat (1995)/heat.mp4", movie("Heat", 1995, "heat")),
        ("movies/Heat (1995)/heat.mov", movie("Heat", 1995, "heat")),
        ("movies/Heat (1995)/heat.MKV", movie("Heat", 1995, "heat")),
        ("movies/Heat (1995)/heat.Mp4", movie("Heat", 1995, "heat")),
        (
            "movies/Heat (1995)/heat.1080p.mkv",
            movie("Heat", 1995, "heat.1080p"),
        ),
        (
            "shows/Dark (2017)/Season 1/S01E01.MOV",
            episode("Dark", 2017, 1, 1, "S01E01"),
        ),
    ]);
}

#[test]
fn other_extensions_are_unassigned() {
    use UnassignedReason::UnsupportedExtension;
    check(&[
        (
            "movies/Heat (1995)/heat.avi",
            unassigned(UnsupportedExtension),
        ),
        (
            "movies/Heat (1995)/heat.mkv.part",
            unassigned(UnsupportedExtension),
        ),
        ("movies/Heat (1995)/heat", unassigned(UnsupportedExtension)),
        ("movies/Heat (1995)/.mkv", unassigned(UnsupportedExtension)),
        (
            "shows/Dark (2017)/Season 1/S01E01.srt",
            unassigned(UnsupportedExtension),
        ),
    ]);
}
