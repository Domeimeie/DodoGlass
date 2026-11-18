# Asset server

**Status: partly built.** See [Built so far](#built-so-far).

The asset server owns the **catalogue**: which Movies, Shows, Seasons and Episodes exist, their
descriptive information (name, year, imdbID, description, image) and which Media files belong to
them. It plays both the *Library service* and the *Metadata service* of the
[system architecture](architecture.md); they can be split later if enrichment needs its own scaling.

Terms are defined in `GLOSSARY.md` at the repository root; the decisions behind this page are recorded
in `docs/adr/` (ADR 0001–0004).

| | |
|---|---|
| Source | `application/asset-server/` (Rust, axum, Diesel) |
| Database | SQLite through Diesel; Postgres later (ADR 0002). Until then only **one** instance may run |
| Metadata | [TMDB](https://www.themoviedb.org), one configurable language (default `en-US`) |
| Images | bucket `assets`, original size from TMDB |
| Runs | natively, `go-task` task, like the transcoder; migrations run at startup |

## Built so far

The tracer bullet (a Movie upload becomes listable), then Shows.

- **Ingest listener** on `$share/assets/dodoglass/ingest`. It reads the standard S3 part of each
  event, URL-decodes the keys, handles several records per message, and acknowledges a message only
  after it is stored.
- **Placement of Movies:** `movies/<Name> (<Year>)/<label>.<ext>` creates the Movie (or finds it) and
  a **Processing** Media file whose id is the shared crate's task id. Keys are normalised to NFC.
- **Placement of Episodes:** `shows/<Name> (<Year>)/Season <n>/S<nn>E<nn>…` (or `Specials/`, Season
  0) finds or creates the Show (by folder name and year), its Season (by number) and its Episode (by
  number), and a Processing Media file attached to the Episode. Every other upload is stored without
  a Movie or Episode (Unassigned).
- **Several Media files per Movie:** each one's Label is its file name without the extension, made
  unique within the Movie (`matrix.mkv` + `matrix.mp4` → "matrix", "matrix (mp4)"; a number is
  added if that is taken too). The first one uploaded is the **Default media file**. A re-upload to
  the same path keeps the id, Label and default flag and goes back to Processing. Labels and the
  default are kept per *owner* of the Media file (`media_file.is_default`, unique indexes per
  owner), so Movies and Episodes follow the same rules.
- **Reconciler:** a Processing Media file becomes **Ready** when `final/<id>/source.json` exists, at
  startup and every `--reconcile-interval` seconds (default 300). Its manifest keys follow the
  `formats` in the marker.
- **Read API:** `GET /movies` lists the Movies that are not Hidden, with their Ready Media files:
  `[{"name", "year", "media_files": [{"id", "label", "default", "manifests": {"hls", "dash"}}]}]`,
  the Media files ordered by Label.
  `GET /shows` → `[{"id", "name", "year"}]`, `GET /shows/{id}/seasons` → `[{"number"}]`,
  `GET /shows/{id}/seasons/{number}/episodes` → `[{"number", "media_files": [...]}]` (Media files as
  for Movies). Hidden applies at every level: an Episode needs a Ready Media file, a Season a listed
  Episode, a Show a listed Season; a Hidden or unknown Show or Season answers 404.
- **Unassigned media files:** an upload outside the layout is stored without a Movie or Episode,
  goes through Processing and Ready like any other, and is never in the read API.
- **Admin API** (under `/admin`, no authentication, localhost only):
  `GET /admin/unassigned-media-files` → `[{"id", "path", "reason", "state"}]` by path, the reason
  taken from the path (`no_year`, `unknown_root_folder`, `wrong_depth`, `unsupported_extension`,
  `unknown_season_folder`, `no_episode_code`, `season_mismatch`).
  `POST /admin/unassigned-media-files/{id}/attach` with `{"movie": {"name", "year"}}` or
  `{"episode": {"show", "year", "season", "number"}}` (folder names and years, as in the upload
  layout) attaches it to that existing Movie or Episode → `{"id", "label"}`. The Label follows the
  upload rule ("matrix (mkv)" if "matrix" is taken); it becomes the default only if the owner had no
  Media file. 404 if the Media file is not Unassigned or the target does not exist. A re-upload to
  the same path keeps it attached.
- **Database:** SQLite via Diesel, migrations in portable SQL run at startup. Tables so far: `movie`
  (with `folder_name`/`folder_year`, the placement identity, apart from the displayed `name`/`year`)
  and `media_file`; `show` (same folder identity), `season`, `episode`, and `media_file.episode_id`.

Not built yet: Failed and Missing, a new default when the default goes away, status hints,
TMDB, images and the rest of the admin API.

**Running it:** `go-task infra:up`, `go-task transcoder:serve-s3`, `go-task asset-server:serve`
(HTTP on `127.0.0.1:8090`), then for example
`go-task infra:upload KEY="movies/The Matrix (1999)/matrix.mkv" -- application/transcoder/sample.mp4`
and `curl localhost:8090/movies`. Settings: `go-task asset-server:serve -- --help`, all also as
`ASSETS_*` environment variables. Code layout and how to write tests:
`application/asset-server/README.md`.

## Upload layout

Where a video is uploaded in `ingest` decides what it is (ADR 0003). The year is **required**.

```
movies/<Name> (<Year>)/<label>.<ext>
shows/<Name> (<Year>)/Season <n>/S<nn>E<nn>[ anything].<ext>
shows/<Name> (<Year>)/Specials/S00E<nn>[ anything].<ext>
```

| Upload | Becomes |
|---|---|
| `movies/The Matrix (1999)/matrix.mkv` | Movie *The Matrix* (1999), Media file labelled "matrix" |
| `movies/The Matrix (1999)/matrix-directors-cut.mkv` | second Media file of the same Movie |
| `shows/Breaking Bad (2008)/Season 01/S01E02 - Cat's in the Bag.mkv` | Show *Breaking Bad* (2008), Season 1, Episode 2 |
| `shows/Breaking Bad (2008)/Specials/S00E01.mkv` | Season 0 (specials), Episode 1 |
| `movies/The Matrix/matrix.mkv` (no year), `misc/clip.mp4`, `Season 01/S02E01.mkv` | **Unassigned media file**, attached by an admin |

Nothing that is uploaded is rejected: what does not fit waits for an admin instead of being lost.

## How a Media file gets in

```mermaid
flowchart TB
    Up(["upload to ingest<br/>movies/The Matrix (1999)/matrix.mkv"]) -.->|ObjectCreated| Topic{{"dodoglass/ingest"}}
    Topic -->|"$share/transcoders"| T[Transcoder]
    Topic -->|"$share/assets"| Id

    subgraph AS["Asset server"]
        Id["task id from key<br/>(shared crate, ADR 0004)"] --> Parse{"path fits the<br/>upload layout?"}
        Parse -->|yes| Place["find or create Movie<br/>or Show → Season → Episode<br/>Media file: Processing, Label from file name"]
        Parse -->|no| Unassigned["Unassigned media file"]
        Rec["reconcile final/*/source.json<br/>at startup + periodically (ADR 0001)"]
        Hint["status hint:<br/>completed / failed"]
        Ready["Ready"]
        Failed["Failed"]
        Missing["Missing"]
    end

    T -->|"final/&lt;id&gt;/ + source.json"| Final[("bucket final")]
    T -.->|dodoglass/status| Hint
    Hint -->|completed| Rec
    Final --> Rec
    Rec --> Ready & Missing
    Hint -->|failed| Failed
    Place & Unassigned & Ready & Failed & Missing --> DB[("SQLite")]
```

1. The asset server subscribes to the ingest events as its own group, so it and the transcoders each
   get every upload. From the key it computes the **same task id** as the transcoder, with the function
   from a crate both depend on.
2. It places the Media file in the catalogue from the path (or makes it an Unassigned media file),
   state **Processing**.
3. A Media file is **Ready** only when `final/<id>/source.json` exists. Status messages are
   best-effort (`try_publish`) and missed while the asset server is down, so they only trigger an early
   check; the reconcile at startup and every few minutes is what counts. `source.json` records the
   input as `{bucket, key}`, so the reconcile can also place Media files it never saw an event for.

### Media file states

```mermaid
stateDiagram-v2
    [*] --> Processing: upload event
    [*] --> Ready: reconcile finds source.json for an unknown key
    Processing --> Ready: source.json found
    Processing --> Failed: failed status, or > 24 h with input still in ingest
    Failed --> Processing: upload to the same path again
    Ready --> Processing: upload to the same path again (new version)
    Ready --> Missing: output gone from final
    Missing --> Ready: output back
    Ready --> [*]: admin removes it
    Failed --> [*]: admin removes it
    Missing --> [*]: admin removes it
```

- Uploading to the **same path** again is a new version of the same Media file: its Movie or Episode,
  Label and Locked fields stay.
- Only an admin removes a Media file; the asset server then deletes `final/<id>/` and the row. Output
  that disappears for any other reason makes it **Missing**, never deleted.
- A Movie or Episode without a Ready Media file is **Hidden**, not deleted, so a re-upload comes back
  with its fixes. Shows and Seasons are hidden when none of their Episodes are visible.
- One Media file per Movie or Episode is the **Default media file**. When it goes away, the oldest
  remaining Ready one takes over.
- **Labels** are unique per Movie or Episode: `matrix.mkv` and `matrix.mp4` become "matrix" and
  "matrix (mp4)". An admin can rename them.

## How imdbID, name, description and image get set

```mermaid
flowchart TB
    New["new Movie or Show<br/>(name + year from the folder)"] --> Search["TMDB search: name + year"]
    Search -->|"TMDB down / rate limited"| Retry["retry with backoff, up to ~1 h<br/>parsed name shown meanwhile"] --> Search
    Search -->|"exactly one result:<br/>same year, same name"| Match["Match: name, year, imdbID, TMDB id,<br/>description, poster"]
    Search -->|"none or several"| Review["Needs review<br/>top 5 candidates kept"]
    Review -->|"admin picks one"| Match
    Match --> Cascade["Seasons + Episodes matched by number<br/>not in TMDB: number as name, no review"]
    Match & Cascade -->|"download original image"| Assets[("bucket assets")]
    Edit["admin edits a field"] --> Lock["Locked field:<br/>never overwritten"]
    Refresh["admin: refresh / pick another Match"] --> Search
```

- **Automatic Match** only when exactly one TMDB result has the same year and the same name (ignoring
  case and punctuation). A wrong automatic Match is worse than one waiting for review.
- **Shows** are searched by first-air year. Their Seasons and Episodes follow by number, without a
  search or review of their own; picking another Match for the Show refills them too.
- **Fetched once**, when an entry first appears, and again only when an admin asks. No periodic
  refresh.
- **Locks are per field.** Picking another Match or refreshing refills every field that is not
  locked.
- **No image** on TMDB: none is stored. A Season falls back to its Show's poster, an Episode to its
  Season's. Placeholders are the frontend's job.

| Level | Fields from TMDB |
|---|---|
| Movie | name, year, imdbID, TMDB id, description, poster |
| Show | name, first-air year, imdbID, TMDB id, description, poster |
| Season | number, name, description, poster |
| Episode | number, name, description, still, air date, imdbID |

## Data model

```mermaid
erDiagram
    MOVIE ||--o{ MEDIA_FILE : has
    SHOW ||--|{ SEASON : has
    SEASON ||--|{ EPISODE : has
    EPISODE ||--o{ MEDIA_FILE : has

    MOVIE {
        int id PK
        string name
        int year
        string imdb_id
        int tmdb_id
        text description
        string image_key
        string match_state "matched, needs_review, retrying"
        text locked_fields
    }
    SHOW {
        int id PK
        string name
        int year "first-air year"
        string imdb_id
        int tmdb_id
        text description
        string image_key
        string match_state
        text locked_fields
    }
    SEASON {
        int id PK
        int show_id FK
        int number "0 = specials"
        string name
        text description
        string image_key "null: Show poster"
        text locked_fields
    }
    EPISODE {
        int id PK
        int season_id FK
        int number
        string name
        text description
        string image_key "null: Season poster"
        date air_date
        string imdb_id
        text locked_fields
    }
    MEDIA_FILE {
        string id PK "task id"
        string source_key UK
        int movie_id FK "null if unassigned"
        int episode_id FK "null if unassigned"
        string label "unique per Movie or Episode"
        bool is_default "one per Movie or Episode"
        string state "processing, ready, failed, missing"
        string hls_manifest
        string dash_manifest
        text error
    }
```

A Media file belongs to a Movie **or** an Episode, or to neither while it is unassigned. The Default
media file is a flag on the Media file rather than a column on its owner, so Movies and Episodes share
one rule; partial unique indexes keep it to one per owner. Migrations
stay within SQL that SQLite and Postgres both accept (ADR 0002).

## API

**Read API** (for the frontends, through the API backend):

- Lists only entries that are not Hidden. Entries in Needs review are shown with the name and year
  from the folder, so a movie can be watched before its metadata is fixed.
- Per Media file: its Label, whether it is the default, and the **manifest keys**
  (`final/<id>/hls/master.m3u8`, `final/<id>/dash/manifest.mpd`). Delivering the streams is the
  Playback service's job.
- `GET /images/<key>` streams an image from `assets`.

**Admin API** under `/admin/...`: list Needs review and Unassigned media files, pick a Match, edit
fields (which locks them), attach an Unassigned media file, set the Default media file, rename a
Label, remove a Media file, refresh.

## Changes outside the asset server

| Where | Change |
|---|---|
| Transcoder | task id = readable prefix + 8 hex of SHA-256 over the NFC-normalised key (ADR 0004), moved into a shared crate |
| Transcoder | `source.json` stores `input` as `{"bucket": …, "key": …}` |
| `infra/init.sh` | creates the bucket `assets` |
| `final` bucket | existing outputs deleted and re-uploaded (new ids, no migration) |

## Open points

- **No authentication** on the admin API: reachable from localhost only, like the development setup
  of MQTT and RustFS. Real authentication comes with the User service.
- **One instance** while the database is SQLite.
- **Several languages** of metadata at once, **periodic refresh** (episodes described after upload),
  **image resizing** and **typed attributes** of Media files (edition, audio language) are left for
  later.
