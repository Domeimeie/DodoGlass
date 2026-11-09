# Setup

Step by step from a fresh **Fedora** machine to a working development environment: RustFS and
Mosquitto in Docker, the transcoder native. Tested on Fedora 44. What the parts are and why they are
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
event rule on ingest: set (.mp4 .mov .mkv -> MQTT topic dodoglass/ingest)
init-1 exited with code 0
```

On later starts it says `exists` instead of `created`.

## 4. Check that it works

**Web console:** open <http://localhost:9001>, log in with `rustfsadmin` / `rustfsadmin`. The buckets
`ingest` and `final` are listed.

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
go-task infra:ls -- final        # sample/hls/…, sample/dash/…, sample/source.json
```

Upload the same file again: `skipped`. Ctrl+C stops the transcoder (running tasks finish first).
`RUST_LOG=info,transcoder::upload=debug` also logs every uploaded file.

## Everyday use

All from the repository root:

| Command | What it does |
|---|---|
| `go-task infra:up` | Start everything, set up buckets and the event rule |
| `go-task infra:down` | Stop everything; bucket contents are kept |
| `go-task infra:reset` | Stop and **delete all bucket contents**, then start fresh (asks first) |
| `go-task infra:watch` | Print every MQTT message |
| `go-task infra:upload -- <file>` | Upload a video to `ingest` |
| `go-task infra:ls [-- <bucket>]` | List the buckets |
| `go-task transcoder:serve-s3` | Run the transcoder on MQTT + RustFS |
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
