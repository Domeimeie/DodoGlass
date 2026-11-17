//! The upload path parser: where an `ingest` key puts its Media file in the
//! catalogue, following the upload layout (ADR 0003).

/// Where an upload belongs in the catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Movie {
        name: String,
        year: u16,
        label: String,
    },
    Episode {
        show: String,
        year: u16,
        season: u32,
        episode: u32,
        label: String,
    },
    Unassigned(UnassignedReason),
}

/// Why an upload does not follow the upload layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnassignedReason {
    /// The Movie or Show folder is not `<Name> (<Year>)`.
    NoYear,
    /// The key does not start with `movies/` or `shows/`.
    UnknownRootFolder,
    /// The file is not directly in a Movie folder or a Season folder.
    WrongDepth,
    /// The file is not `.mp4`, `.mov` or `.mkv`.
    UnsupportedExtension,
    /// The folder inside a Show is neither `Season <n>` nor `Specials`.
    UnknownSeasonFolder,
    /// The Episode file name does not start with `S<nn>E<nn>`.
    NoEpisodeCode,
    /// The season in the episode code is not the folder's season.
    SeasonMismatch,
}

/// Places the upload at `key` in the catalogue.
pub fn place_upload(key: &str) -> Placement {
    match place(key) {
        Ok(placement) => placement,
        Err(reason) => Placement::Unassigned(reason),
    }
}

fn place(key: &str) -> Result<Placement, UnassignedReason> {
    use UnassignedReason::*;
    let parts: Vec<&str> = key.split('/').collect();
    match parts.as_slice() {
        ["movies", folder, file] if !file.is_empty() => {
            let label = video_stem(file).ok_or(UnsupportedExtension)?;
            let (name, year) = name_and_year(folder).ok_or(NoYear)?;
            Ok(Placement::Movie {
                name: name.into(),
                year,
                label: label.into(),
            })
        }
        ["shows", folder, season_folder, file] if !file.is_empty() => {
            let label = video_stem(file).ok_or(UnsupportedExtension)?;
            let (show, year) = name_and_year(folder).ok_or(NoYear)?;
            let season = season_number(season_folder).ok_or(UnknownSeasonFolder)?;
            let (code_season, episode) = episode_code(label).ok_or(NoEpisodeCode)?;
            if code_season != season {
                return Err(SeasonMismatch);
            }
            Ok(Placement::Episode {
                show: show.into(),
                year,
                season,
                episode,
                label: label.into(),
            })
        }
        ["movies" | "shows", ..] => Err(WrongDepth),
        _ => Err(UnknownRootFolder),
    }
}

/// `<Name> (<Year>)` → name and year; the year has four digits.
fn name_and_year(folder: &str) -> Option<(&str, u16)> {
    let (name, year) = folder.strip_suffix(')')?.rsplit_once(" (")?;
    if name.trim().is_empty() || year.len() != 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((name, year.parse().ok()?))
}

/// `Season <n>` → n (`Season 1` and `Season 01` alike), `Specials` → 0.
fn season_number(folder: &str) -> Option<u32> {
    if folder == "Specials" {
        return Some(0);
    }
    let (number, rest) = leading_number(folder.strip_prefix("Season ")?)?;
    rest.is_empty().then_some(number)
}

/// `S<nn>E<nn>[ anything]`, in any case → season and episode.
fn episode_code(file_stem: &str) -> Option<(u32, u32)> {
    let rest = file_stem.strip_prefix(['S', 's'])?;
    let (season, rest) = leading_number(rest)?;
    let rest = rest.strip_prefix(['E', 'e'])?;
    let (episode, _anything) = leading_number(rest)?;
    Some((season, episode))
}

/// The decimal number at the start of `text`, and what follows it.
fn leading_number(text: &str) -> Option<(u32, &str)> {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let number = text[..end].parse().ok()?;
    Some((number, &text[end..]))
}

/// The extensions of the video files the transcoder takes, matched in any case.
const VIDEO_EXTENSIONS: [&str; 3] = ["mp4", "mov", "mkv"];

/// The file name without its video extension. A name that is only an
/// extension (`.mkv`) is a hidden file without one.
fn video_stem(file_name: &str) -> Option<&str> {
    let (stem, extension) = file_name.rsplit_once('.')?;
    let is_video = VIDEO_EXTENSIONS
        .iter()
        .any(|video| extension.eq_ignore_ascii_case(video));
    (is_video && !stem.is_empty()).then_some(stem)
}
