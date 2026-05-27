#!/usr/bin/env python3
"""Split a location CSV into deterministic shards for resumable downloads."""

from __future__ import annotations

import argparse
import csv
from pathlib import Path


def read_locations(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def is_existing(location_id: str, raw_dirs: list[Path], start: str, end: str) -> bool:
    filename = f"{location_id}_{start}_{end}.json"
    return any((raw_dir / filename).exists() for raw_dir in raw_dirs)


def write_shard(path: Path, fieldnames: list[str], rows: list[dict[str, str]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--prefix", default="locations")
    parser.add_argument("--shards", type=int, required=True)
    parser.add_argument("--exclude-raw-dir", type=Path, action="append", default=[])
    parser.add_argument("--start", default="20200101")
    parser.add_argument("--end", default="20241231")
    args = parser.parse_args()

    if args.shards < 1:
        raise SystemExit("--shards must be at least 1")

    rows = read_locations(args.input)
    if not rows:
        raise SystemExit(f"no locations found in {args.input}")

    fieldnames = list(rows[0].keys())
    filtered = [
        row
        for row in rows
        if not is_existing(row["location_id"], args.exclude_raw_dir, args.start, args.end)
    ]

    shards: list[list[dict[str, str]]] = [[] for _ in range(args.shards)]
    for index, row in enumerate(filtered):
        shards[index % args.shards].append(row)

    for index, shard_rows in enumerate(shards):
        out = args.out_dir / f"{args.prefix}_{index:02d}_of_{args.shards:02d}.csv"
        write_shard(out, fieldnames, shard_rows)
        print(f"wrote {out} rows={len(shard_rows)}")

    print(f"input_rows={len(rows)} filtered_rows={len(filtered)} excluded_rows={len(rows) - len(filtered)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
