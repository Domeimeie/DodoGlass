#!/usr/bin/env python3
"""Publish demo tasks to the transcoder's mock PubSub, and optionally follow them.

The mock PubSub is a folder: a task is a JSON file in <mock-dir>/transcode.tasks/,
status messages are lines in <mock-dir>/transcode.status.jsonl.

Examples:
    scripts/publish_task.py samples/bars_1080p24.mp4
    scripts/publish_task.py samples --follow          # every .mp4 in the folder
    scripts/publish_task.py movie.mp4 --id movie-1 --formats hls
    scripts/publish_task.py /data/in/movie.mp4        # a path only the service can see
"""

import argparse
import json
import os
import sys
import time
from pathlib import Path

TASK_CHANNEL = "transcode.tasks"
STATUS_CHANNEL = "transcode.status"
FINAL_EVENTS = {"completed", "failed", "rejected", "skipped"}


def find_inputs(paths):
    """Files as given; folders expand to the .mp4 files directly inside them."""
    inputs = []
    for p in map(Path, paths):
        if p.is_dir():
            inputs.extend(sorted(f for f in p.iterdir() if f.is_file() and f.suffix.lower() == ".mp4"))
        else:
            inputs.append(p)
    return inputs


def input_path(path):
    """Absolute if the file exists here; otherwise kept as given, assuming it is
    a path the service sees (for example a mounted folder in its container)."""
    if path.exists():
        return str(path.resolve())
    print(f"note: {path} not found here, publishing it as a path inside the service", file=sys.stderr)
    return str(path)


def publish(mock_dir, task):
    """Write to a temporary name, then rename: the service only picks up *.json,
    so it never sees a half-written file."""
    inbox = mock_dir / TASK_CHANNEL
    inbox.mkdir(parents=True, exist_ok=True)
    final = inbox / f"{task['id']}.json"
    tmp = inbox / f".{task['id']}.json.tmp"
    tmp.write_text(json.dumps(task, indent=2) + "\n")
    os.replace(tmp, final)
    return final


def follow(mock_dir, task_ids):
    """Print status messages for the given tasks until each one is finished."""
    status_file = mock_dir / f"{STATUS_CHANNEL}.jsonl"
    pending = set(task_ids)
    offset = status_file.stat().st_size if status_file.exists() else 0
    workers = list((mock_dir / "workers").glob("*.json"))
    if not workers:
        print("note: no worker is registered; start one with: cargo run -- --service", file=sys.stderr)
    print(f"following {len(pending)} task(s), Ctrl+C to stop…")
    while pending:
        if status_file.exists():
            with status_file.open() as f:
                f.seek(offset)
                for line in f:
                    message = json.loads(line)
                    if message.get("task") in pending:
                        print_status(message)
                        if message["event"] in FINAL_EVENTS:
                            pending.discard(message["task"])
                offset = f.tell()
        time.sleep(0.5)


def print_status(m):
    parts = [time.strftime("%H:%M:%S", time.localtime(m["time_ms"] / 1000)), m["task"], m["event"]]
    for key in ("format", "percent", "manifest", "error"):
        if key in m:
            parts.append(f"{m[key]}%" if key == "percent" else str(m[key]))
    print("  ".join(parts))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("inputs", nargs="+", help="video files, or folders of .mp4 files")
    parser.add_argument("--id", help="task id (only with a single input); default: <file name>-<timestamp>")
    parser.add_argument("--formats", nargs="+", choices=["hls", "dash"], help="default: whatever the service uses")
    parser.add_argument("--mock-dir", type=Path, default=Path("mock-pubsub"), help="default: mock-pubsub")
    parser.add_argument("--follow", action="store_true", help="print status messages until the tasks finish")
    args = parser.parse_args()

    inputs = find_inputs(args.inputs)
    if not inputs:
        parser.error("no .mp4 files found")
    if args.id and len(inputs) > 1:
        parser.error("--id only works with a single input")

    stamp = time.strftime("%Y%m%d-%H%M%S")
    task_ids = []
    for path in inputs:
        task = {"id": args.id or f"{path.stem}-{stamp}", "input": input_path(path)}
        if args.formats:
            task["formats"] = args.formats
        written = publish(args.mock_dir, task)
        task_ids.append(task["id"])
        print(f"published {task['id']} → {written}")

    if args.follow:
        try:
            follow(args.mock_dir, task_ids)
        except KeyboardInterrupt:
            print()


if __name__ == "__main__":
    main()
