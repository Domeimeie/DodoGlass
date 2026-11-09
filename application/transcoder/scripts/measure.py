#!/usr/bin/env python3
"""Measure a running process and all its child processes (e.g. the transcoder
and the ffmpeg runs it starts) until it exits or Ctrl+C.

For every process seen: peak memory (RSS) and bytes read from / written to
disk. Linux only, standard library only; reads /proc.

    scripts/measure.py <pid>            # e.g. pgrep -f 'transcoder --service'
    scripts/measure.py <pid> --interval 0.2

What the numbers mean:
  peak RSS     most memory the process used at once (sampled; very short peaks can be missed)
  disk read    read_bytes from /proc/<pid>/io: data actually fetched from a disk
  disk write   write_bytes from /proc/<pid>/io: data actually sent to a disk

Network traffic is not per process in /proc (and socket reads with recv() are
not in rchar either). Measure it where it arrives, e.g. at the storage
container: docker stats --no-stream <container>  (NET I/O column).
"""

import argparse
import os
import signal
import time


def children(pid):
    """Direct and indirect child processes."""
    found, todo = set(), [pid]
    while todo:
        p = todo.pop()
        try:
            for task in os.listdir(f"/proc/{p}/task"):
                with open(f"/proc/{p}/task/{task}/children") as f:
                    for c in f.read().split():
                        if int(c) not in found:
                            found.add(int(c))
                            todo.append(int(c))
        except OSError:
            pass
    return found


def sample(pid):
    """(name, rss_bytes, read_bytes, write_bytes) or None if the process is gone."""
    try:
        with open(f"/proc/{pid}/comm") as f:
            name = f.read().strip()
        rss = 0
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    rss = int(line.split()[1]) * 1024
        io = {}
        with open(f"/proc/{pid}/io") as f:
            for line in f:
                key, value = line.split(":")
                io[key] = int(value)
        return name, rss, io.get("read_bytes", 0), io.get("write_bytes", 0)
    except (OSError, ValueError):
        return None


def human(n):
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024 or unit == "GB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("pid", type=int)
    parser.add_argument("--interval", type=float, default=0.5, help="seconds between samples (default 0.5)")
    args = parser.parse_args()

    stats = {}  # pid -> dict(name, peak_rss, write, read, first, last)
    stop = False

    def on_signal(*_):
        nonlocal stop
        stop = True

    signal.signal(signal.SIGINT, on_signal)
    signal.signal(signal.SIGTERM, on_signal)
    started = time.time()
    print(f"measuring pid {args.pid} and its children every {args.interval}s, Ctrl+C to stop…")

    while not stop:
        if sample(args.pid) is None:
            break
        for pid in {args.pid} | children(args.pid):
            s = sample(pid)
            if s is None:
                continue
            name, rss, read, write = s
            now = time.time() - started
            st = stats.setdefault(pid, {"name": name, "peak_rss": 0, "write": 0, "read": 0, "first": now})
            st["peak_rss"] = max(st["peak_rss"], rss)
            # Counters only grow; keep the last value seen before the process exits.
            st["read"], st["write"], st["last"] = read, write, now
        time.sleep(args.interval)

    print(f"\n{'pid':>8}  {'process':<16} {'seen (s)':>14} {'peak RSS':>10} {'disk read':>11} {'disk write':>11}")
    for pid, st in sorted(stats.items(), key=lambda kv: kv[1]["first"]):
        seen = f"{st['first']:.0f}–{st['last']:.0f}"
        print(f"{pid:>8}  {st['name']:<16} {seen:>14} {human(st['peak_rss']):>10} {human(st['read']):>11} {human(st['write']):>11}")
    ffmpeg = [s for s in stats.values() if s["name"] == "ffmpeg"]
    if ffmpeg:
        print(f"\nffmpeg runs: {len(ffmpeg)}, largest peak RSS {human(max(s['peak_rss'] for s in ffmpeg))}, "
              f"read from disk {human(sum(s['read'] for s in ffmpeg))}, written to disk {human(sum(s['write'] for s in ffmpeg))}")


if __name__ == "__main__":
    main()
