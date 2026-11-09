# DodoGlass Transcoder

An ffmpeg wrapper that turns videos into **HLS** and **DASH** with several quality levels, each
format in one manifest. Give it one file or a whole folder; several videos are transcoded in
parallel. Progress is shown in a full-screen terminal interface, or as plain text when run headless.

With `--service` it runs unattended instead: it registers on PubSub, waits for tasks, transcodes them
and publishes status. That is how it runs on the platform, later as a Docker container. On MQTT and
RustFS it transcodes every video uploaded to the `ingest` bucket into the `final` bucket, streaming
in and out: the video never lands on the transcoder's disk ([Efficiency](../../docs/architecture/efficiency.md)).

How it works inside: see the [Transcoder architecture page](../../docs/architecture/transcoder.md),
also served by the docs site (`go-task docs:serve`).

## Requirements

- Rust (stable, edition 2024)
- `ffmpeg` and `ffprobe` on the `PATH`, with `libx264` and `aac`
- Python 3 for `scripts/publish_task.py` (standard library only)
- [go-task](https://taskfile.dev) for the tasks below (optional; on Fedora the command is `go-task`)

## Quick start

From this folder:

```bash
go-task samples                # create a folder of test videos (samples/)
cargo run -- samples           # transcode all of them, 4 at a time
```

Or a single file:

```bash
go-task sample                 # create one 30 s test video (sample.mp4)
cargo run -- sample.mp4
```

Output lands in `out/<video name>/`:

```bash
vlc out/sample/hls/master.m3u8
vlc out/sample/dash/manifest.mpd
```

## Usage

```
cargo run -- [OPTIONS] <INPUT>
```

| Option | Meaning |
|---|---|
| `<INPUT>` | A video file, or a folder: every `.mp4`, `.mov` and `.mkv` directly inside it is transcoded (subfolders are not searched) |
| `-c, --config <FILE>` | Transcoding config (TOML). Without it, the built-in `ladder.toml` is used |
| `-o, --out <DIR>` | Output folder. Each video gets its own subfolder `<DIR>/<video name>`. Default: `out` |
| `-f, --format <hls\|dash\|both>` | Which formats to produce. Default: `both` |
| `-t, --threads <N>` | How many transcodes run at the same time. Default: `4` |
| `--headless` | Plain text output instead of the interface. Automatic when not run in a terminal |
| `--dry-run` | Print the ffmpeg commands without running them |
| `--print-config` | Print the built-in config, as a starting point for your own |
| `--service` | Run as an unattended service (see [Service mode](#service-mode)) |

Every option can also be set through an environment variable; `--help` shows which.

Examples:

```bash
cargo run -- samples -t 2                       # a folder, 2 at a time
cargo run -- movie.mp4 -c my-ladder.toml        # your own ladder
cargo run -- movie.mp4 -f hls -o /tmp/videos    # HLS only, into /tmp/videos/movie/
cargo run -- samples --headless                 # for scripts and servers
cargo run -- samples --dry-run                  # see what ffmpeg would run
```

### The interface

- **Overall:** progress over all jobs.
- **Videos:** one row per video with a progress bar each for HLS and DASH. `↑`/`↓` (or `j`/`k`) scroll
  when there are more videos than fit.
- **Ladder:** the renditions from the config.
- **Total speed:** the combined encoding speed of everything running.
- **ffmpeg log:** every line is prefixed with the video and format it belongs to.

`q` (or `Esc`, `Ctrl+C`) cancels: running transcodes are stopped, queued ones never start. Once
everything is finished, `q` exits.

### Errors

A file that cannot be read is reported and skipped; the rest of the folder still runs. A failing
transcode does not stop the others. The exit code is non-zero if anything failed, was unreadable or
was cancelled.

## Service mode

```bash
go-task serve                                   # or: cargo run -- --service
```

The service registers as a worker, takes tasks from the `transcode.tasks` channel, runs up to
`--threads` of them at once and publishes status to `transcode.status`. There is no TUI; logs go to
stdout.

### On MQTT and RustFS

Start the infrastructure (`go-task infra:up` from the repository root, see the
[setup guide](../../docs/architecture/setup.md)), then:

```bash
go-task serve-s3 -- -t 2                        # MQTT + RustFS, results to the bucket "final"
go-task infra:upload -- application/transcoder/sample.mp4   # from the repository root
```

Each upload of a `.mp4`, `.mov` or `.mkv` to `ingest` becomes a task; the result goes to
`final/<name>/hls/` and `final/<name>/dash/`, and the original is removed from `ingest` (unless
`--keep-input`). Uploading the same file again is skipped; a different file with the same name
replaces the result. Details: [architecture page](../../docs/architecture/transcoder.md#the-mqtt-backend).

### On the mock PubSub

Without a broker, the PubSub can be a **mock** made of files in `mock-pubsub/`. Publish tasks from a
second terminal with the Python script:

```bash
go-task publish -- samples/bars_1080p24.mp4     # one video
go-task publish -- samples --follow             # every .mp4 in samples/, then follow their status
scripts/publish_task.py movie.mp4 --id movie-1 --formats hls
```

`--follow` prints each status message until the tasks finish. `go-task status` follows the whole
status channel instead.

A task is a JSON file in `mock-pubsub/transcode.tasks/`:

```json
{"id": "movie-1", "input": "/path/to/movie.mp4", "formats": ["hls", "dash"]}
```

Only `input` is required. The output goes to `<out>/<id>/`.

| Service option | Environment variable | Default |
|---|---|---|
| `--service` | `TRANSCODER_SERVICE=true` | off |
| `--pubsub mock\|mqtt` | `TRANSCODER_PUBSUB` | `mock` |
| `--mqtt-url`, `--mqtt-topic`, `--mqtt-group` | `TRANSCODER_MQTT_*` | `mqtt://localhost:1883`, `dodoglass/ingest`, `transcoders` |
| `--s3-endpoint`, `--s3-access-key`, `--s3-secret-key` | `TRANSCODER_S3_*` | none |
| `--output-bucket` | `TRANSCODER_OUTPUT_BUCKET` | none: write to `--out` |
| `--keep-input` | `TRANSCODER_KEEP_INPUT` | off: delete inputs after a completed or skipped task |
| `--mock-dir` | `TRANSCODER_MOCK_DIR` | `mock-pubsub` |
| `--worker-id` | `TRANSCODER_WORKER_ID` | `<hostname>-<pid>` |
| `--log-format text\|json` | `TRANSCODER_LOG_FORMAT` | `text` |
| `-t, --threads` (tasks at once) | `TRANSCODER_THREADS` | `4` |
| `-o, --out` | `TRANSCODER_OUT` | `out` |
| `-f, --format` (if a task doesn't say) | `TRANSCODER_FORMAT` | `both` |
| `-c, --config` | `TRANSCODER_CONFIG` | built-in ladder |
| log level | `RUST_LOG` (`debug` shows ffmpeg's output) | `info` |

Stopping: the first Ctrl+C or SIGTERM stops taking new tasks and lets running ones finish; a second
one cancels them.

Message formats, the mock's folder layout and container notes: see the
[architecture page](../../docs/architecture/transcoder.md#service-mode).

## Config

`ladder.toml` is both the built-in default and an example. Copy it to make your own:

```bash
cargo run -- --print-config > my-ladder.toml
```

```toml
segment_duration = 4        # segment length in seconds
preset = "veryfast"         # x264 speed vs. compression
audio_bitrate_kbps = 128    # AAC stereo
# fps = 30                  # optional frame rate for all renditions (default: keep the source's)

[[rendition]]               # one block per quality version
name = "1080p"              # folder name, must be unique
height = 1080               # picture height in pixels, must be even
bitrate_kbps = 5000         # average video bitrate
# maxrate_kbps = 5350       # optional peak cap   (default: bitrate + 7 %)
# bufsize_kbps = 7500       # optional rate buffer (default: 1.5 × bitrate)
# fps = 15                  # optional frame rate for this rendition only
```

- Several renditions may share a resolution as long as their names differ, for example a
  `1080p_high` and a `1080p_low`.
- Heights are applied literally, also to portrait video: "1080p" of a 1080×1920 video is 608×1080.
- The program warns when a rendition is taller than the source (upscaling) or has a higher frame rate
  than the source (duplicated frames).

## Output

```
out/<video name>/
├── hls/master.m3u8 + <rendition>/index.m3u8 + seg_NNN.ts
└── dash/manifest.mpd + init-streamN.m4s + chunk-streamN-NNNNN.m4s
```

Each run replaces the previous output of the same video and format.

## Test videos

`go-task samples` creates `samples/` with videos that each test something different:

| File | Tests |
|---|---|
| `bars_1080p24.mp4` | standard 1080p, 24 fps |
| `mandelbrot_720p30.mp4` | constant fine detail, hard to compress |
| `motion_1080p60.mp4` | 60 fps source |
| `life_480p25_noaudio.mp4` | no audio track, upscaling |
| `portrait_1080x1920.mp4` | vertical video |
| `gradients_1440p24.mp4` | larger than 1080p, smooth gradients (banding) |

## Development tasks

Run from this folder with `go-task <name>`, or from the repository root with
`go-task transcoder:<name>`:

| Task | What it does |
|---|---|
| `build` | Debug build |
| `release` | Optimized build in `target/release/transcoder` |
| `run -- <args>` | Run with arguments, e.g. `go-task run -- samples -t 2` |
| `serve -- <args>` | Run as a service on the mock PubSub |
| `serve-s3 -- <args>` | Run as a service on MQTT + RustFS (start `infra:up` first) |
| `publish -- <args>` | Publish tasks with `scripts/publish_task.py`, e.g. `go-task publish -- samples --follow` |
| `status` | Follow the mock status channel |
| `sample` | Create the 30 s test video `sample.mp4` (skipped if it exists) |
| `samples` | Create the test videos in `samples/` (existing files are kept) |
| `test` | Run the tests |
| `lint` | Clippy, with warnings treated as errors |
| `fmt` | Format the code |
| `check` | Formatting check, lint and tests: run before committing |
| `install` | Install as the `transcoder` command in `~/.cargo/bin` |
| `clean` | Remove build artefacts, `out/` and `mock-pubsub/` |
