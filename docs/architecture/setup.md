# Setup

Step by step from a fresh **Fedora** machine to a working development environment: RustFS and
Mosquitto in Docker, the transcoder and the asset server native. Tested on Fedora 44. What the parts are and why they are
configured this way: see [Infrastructure](infrastructure.md).

## 1. Install the tools

```bash
# Docker (Fedora's packages) and Compose
sudo dnf install moby-engine docker-compose
sudo systemctl enable --now docker
sudo usermod -aG docker $USER        # then log out and in again, so docker works without sudo

# Rust (with formatter and linter for `go-task check`), go-task, MQTT command-line clients
sudo dnf install rust cargo rustfmt clippy go-task mosquitto
```

**ffmpeg with libx264.** Fedora's own `ffmpeg-free` does not include the H.264 encoder the
transcoder needs. Install the full ffmpeg from [RPM Fusion](https://rpmfusion.org/Configuration):

```bash
sudo dnf install https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm
sudo dnf swap ffmpeg-free ffmpeg --allowerasing
ffmpeg -hide_banner -encoders | grep libx264      # must print a line
```

The `mosquitto` package is only needed for its clients `mosquitto_pub` and `mosquitto_sub`; the
broker itself runs in Docker. Do **not** start the native broker (`mosquitto` or
`systemctl start mosquitto`): it would block port 1883.

## 2. Get the code

```bash
git clone git@github.com:Domeimeie/DodoGlass.git
cd DodoGlass
```

## 3. Start the infrastructure

```bash
go-task infra:up
```

This starts RustFS and Mosquitto in the background, then runs the one-shot `init` container, which
creates the buckets and the event rule. The first start downloads the images. Expected output at the
end:

```
waiting for RustFS at http://rustfs:9000 ...
bucket ingest: created
bucket final: created
bucket assets: created
folder ingest/movies/: created
folder ingest/shows/: created
event rule on ingest: set (.mp4 .mov .mkv -> MQTT topic dodoglass/ingest)
init-1 exited with code 0
```

On later starts it says `exists` instead of `created`.

## 4. Check that it works

**Web console:** open <http://localhost:9001>, log in with `rustfsadmin` / `rustfsadmin`. The buckets
`ingest`, `final` and `assets` are listed; `ingest` contains the empty folders `movies/` and `shows/`
to upload into (step 7).

**Events:** in one terminal, print every MQTT message:

```bash
go-task infra:watch          # or: mosquitto_sub -t 'dodoglass/#' -v
```

In a second terminal, create a test video and upload it in the web console (bucket `ingest` →
Upload):

```bash
cd application/transcoder
go-task sample               # creates sample.mp4
```

A JSON message on `dodoglass/ingest` appears in the first terminal. Uploading a `.txt` file produces
no message. Instead of the web console, `go-task infra:upload -- application/transcoder/sample.mp4`
uploads from the command line (no AWS CLI needed: it uses the `init` container).

## 5. Run the transcoder on it

In a third terminal:

```bash
go-task transcoder:serve-s3 -- -t 2
```

It connects to Mosquitto and RustFS (`localhost`, development credentials from `Taskfile.yml`),
registers and waits. Upload a video to `ingest`: the log shows `task accepted`, progress, `uploaded`,
`job done` for HLS and DASH and `task completed`; the first terminal shows the status messages on
`dodoglass/status`. The result:

```bash
go-task infra:ls -- final        # sample-de8f39c7/hls/…, …/dash/…, …/source.json
```

The output folder is the **task id**: the file name plus a short hash of the full upload path, so two
uploads never share a folder (see [Transcoder](transcoder.md)).

After the task, `sample.mp4` is gone from `ingest`: a transcoded input is consumed (`--keep-input`
keeps it). Upload the same file again: `skipped`, and it is removed as well. Ctrl+C stops the
transcoder (running tasks finish first).
`RUST_LOG=info,transcoder::upload=debug` also logs every uploaded file.

## 6. Get a TMDB key (optional)

The asset server fills in names, descriptions, imdbIDs and posters from
[TMDB](https://www.themoviedb.org). Without a key everything else works; Movies and Shows then keep
the name and year from their folder.

1. Create an account on themoviedb.org, then **Settings → API → Request an API key**, type
   *Developer*, personal / non-commercial use. As *Application URL* the repository URL is fine.
2. Either value it shows works: the **API key (v3)** or the **API Read Access Token (v4)**.
3. Put it into a git-ignored `.env` next to the asset server, without it ending up in your shell
   history:

```bash
cd application/asset-server
read -rsp 'TMDB key: ' k && printf 'ASSETS_TMDB_API_KEY=%s\n' "$k" > .env && chmod 600 .env && unset k
cd ../..
```

`go-task asset-server:serve` loads this file; a value exported in the shell takes precedence.

## 7. Run the asset server

In another terminal, **from the repository root**:

```bash
go-task asset-server:serve
```

It connects to Mosquitto and RustFS, creates its database (`application/asset-server/assets.db`,
migrations run at startup), looks for finished outputs in `final` and listens on
`http://127.0.0.1:8090`. The log shows `HTTP API listening` and, with a key, a `matched` line per
Movie or Show.

### Upload by path

Where a video is uploaded in `ingest` decides what it is. The year is required.

```bash
go-task infra:upload KEY="movies/The Matrix (1999)/matrix.mkv" -- application/transcoder/sample.mp4
go-task infra:upload KEY="shows/Breaking Bad (2008)/Season 01/S01E02.mkv" -- application/transcoder/sample.mp4
```

| Path | Becomes |
|---|---|
| `movies/<Name> (<Year>)/<label>.<ext>` | a Movie; several files in one folder are several versions, the file name is the Label |
| `shows/<Name> (<Year>)/Season <n>/S<nn>E<nn>[ anything].<ext>` | an Episode of that Season of that Show |
| `shows/<Name> (<Year>)/Specials/S00E<nn>.<ext>` | an Episode of Season 0 |
| anything else (no year, other folder, season mismatch) | an **Unassigned media file**, kept for an admin |

Name and year must be the ones TMDB uses (case and punctuation don't matter), otherwise the entry
lands in *Needs review*. A file at the top level, like `sample.mp4` in step 4, becomes Unassigned.

### See the result

Open in a browser or with `curl -s … | python3 -m json.tool`. There is no page at `/` itself (404):

| URL | Shows |
|---|---|
| `/movies`, `/movies/<id>` | Movies with description, imdbID, poster and their Media files (manifests) |
| `/shows` | Shows with description, imdbID and poster; **no files**: those belong to Episodes |
| `/shows/<id>/seasons` | the Seasons of a Show |
| `/shows/<id>/seasons/<n>/episodes` | the Episodes with name, description, still, air date and their Media files |
| `/images/<image>` | a poster or still; `<image>` is the `image` field, e.g. `posters/….jpg` |
| `/admin/needs-review` | Movies and Shows without a certain Match, with candidates |
| `/admin/unassigned-media-files` | uploads that did not fit the layout, with the reason |
| `/admin/media-files?state=failed,missing` | failed transcodes and vanished outputs |

A video shows up once its transcode is complete (the transcoder's `completed` message, at the latest
at the next reconcile, every 300 s). Correcting matches, editing fields and attaching files: see the
asset server's `README.md` and [Asset server](asset-server.md). The database is a plain SQLite file;
`go-task asset-server:clean` deletes it and the build, for a fresh catalogue.

## Everyday use

All from the repository root:

| Command | What it does |
|---|---|
| `go-task infra:up` | Start everything, set up buckets and the event rule |
| `go-task infra:down` | Stop everything; bucket contents are kept |
| `go-task infra:reset` | Stop and **delete all bucket contents**, then start fresh (asks first) |
| `go-task infra:watch` | Print every MQTT message |
| `go-task infra:upload -- <file>` | Upload a video to `ingest` (top level: becomes Unassigned) |
| `go-task infra:upload KEY="movies/<Name> (<Year>)/<label>.mkv" -- <file>` | Upload under a catalogue path |
| `go-task infra:ls [-- <bucket>]` | List the buckets |
| `go-task transcoder:serve-s3` | Run the transcoder on MQTT + RustFS |
| `go-task asset-server:serve` | Run the asset server, API on `127.0.0.1:8090` |
| `go-task asset-server:clean` | Delete the asset server's database (and build) |
| `go-task infra:logs` | Follow the logs of RustFS and Mosquitto |
| `go-task infra:ps` | Show the containers |

## Troubleshooting

| Problem | Cause and fix |
|---|---|
| `permission denied ... docker.sock` | You are not in the `docker` group yet: `sudo usermod -aG docker $USER`, then log out and in |
| `port is already allocated` for 1883 | A native Mosquitto is running: stop it (Ctrl+C, or `sudo systemctl stop mosquitto`) |
| Mosquitto: `Error opening config file` / permission denied | SELinux; the `:Z` on the volume in `compose.yml` must be there |
| `init` ends with `RustFS did not answer within 60 s` | RustFS failed to start: `go-task infra:logs` and look for errors |
| Upload works, but no MQTT message | Check the file ends in lowercase `.mp4`, `.mov` or `.mkv` (case-sensitive). Check the RustFS log for `mqtt` errors. Run `go-task infra:up` again to reset the rule |
| ffmpeg: `Unknown encoder 'libx264'` | `ffmpeg-free` is installed instead of RPM Fusion's ffmpeg, see step 1 |
| `cargo fmt` / `cargo clippy`: `no such command` | `sudo dnf install rustfmt clippy` |
| Transcoder: `cannot connect to the MQTT broker` / `cannot access bucket final` | Infrastructure not running: `go-task infra:up` |
| A task fails with `412 Precondition Failed` | The video was replaced in `ingest` while it was being transcoded; the new upload has its own task |
| `Task "asset-server:serve" does not exist` | Run it from the repository root, or `go-task serve` inside `application/asset-server/` |
| Asset server: `cannot listen on 127.0.0.1:8090` | Another asset server is already running, or another program uses the port: stop it, or `--http-addr 127.0.0.1:8091` |
| `http://127.0.0.1:8090` shows nothing / 404 | There is no page at `/`; open `/movies` or `/shows` |
| A Movie or Show has no description or poster | No `ASSETS_TMDB_API_KEY` (see step 6), or it is in `/admin/needs-review`: name or year differ from TMDB's |
| A video does not appear at all | Its transcode is not complete yet (`/admin/media-files/<id>`), or its path did not fit: `/admin/unassigned-media-files` |
