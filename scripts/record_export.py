#!/usr/bin/env python3
# Copyright (c) 2026 Metrum AI. All rights reserved.
"""Export all-smi NDJSON recordings as normalized CSV or Parquet.

For zstd recordings, pipe decompressed NDJSON on stdin (for example,
`zstd -dc trace.ndjson.zst | python3 scripts/record_export.py --format csv`).
Gzip inputs are detected by suffix and decompressed directly.
"""

from __future__ import annotations

import argparse
import csv
import gzip
import json
import math
import sys
from typing import IO, Iterable, Iterator


FIELDS = [
    "timestamp",
    "seq",
    "interval_ms",
    "lag_ms",
    "missed_intervals",
    "gap",
    "hostname",
    "section",
    "device_id",
    "device_name",
    "metric",
    "value",
    "unit",
]
SECTIONS = ("gpus", "cpus", "memory", "chassis", "processes", "storage")
IDENTIFIER_FIELDS = {"index", "device_id", "pid", "ppid"}


def iter_rows(stream: Iterable[str]) -> Iterator[dict[str, object]]:
    """Yield one normalized row per numeric metric in a data frame."""
    units: dict[str, str] = {}
    for line_number, line in enumerate(stream, start=1):
        if not line.strip():
            continue
        try:
            frame = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValueError(f"invalid JSON on input line {line_number}: {error}") from error
        if not isinstance(frame, dict) or frame.get("header") or frame.get("index"):
            if isinstance(frame, dict) and frame.get("header"):
                candidate = frame.get("units", {})
                if isinstance(candidate, dict):
                    units = {str(key): str(value) for key, value in candidate.items()}
            continue

        metadata = {
            "timestamp": frame.get("timestamp", ""),
            "seq": frame.get("seq"),
            "interval_ms": frame.get("interval_ms"),
            "lag_ms": frame.get("lag_ms"),
            "missed_intervals": frame.get("missed_intervals"),
            "gap": bool(frame.get("gap", False)),
            "hostname": frame.get("hostname", ""),
        }
        emitted = False
        for section in SECTIONS:
            devices = frame.get(section)
            if not isinstance(devices, list):
                continue
            for device in devices:
                if not isinstance(device, dict):
                    continue
                device_id = device.get("uuid", device.get("device_uuid"))
                if device_id is None:
                    device_id = device.get("index", device.get("pid", ""))
                device_name = device.get("name", device.get("process_name", ""))
                for metric, value in device.items():
                    if metric in IDENTIFIER_FIELDS:
                        continue
                    if isinstance(value, bool) or not isinstance(value, (int, float)):
                        continue
                    if isinstance(value, float) and not math.isfinite(value):
                        continue
                    row = dict(metadata)
                    row.update(
                        section=section,
                        device_id=str(device_id),
                        device_name=str(device_name),
                        metric=metric,
                        value=float(value),
                        unit=units.get(f"{section}.{metric}", ""),
                    )
                    yield row
                    emitted = True
        if metadata["gap"] or not emitted:
            row = dict(metadata)
            row.update(
                section="recording",
                device_id="",
                device_name="",
                metric="gap" if metadata["gap"] else "frame",
                value=1.0 if metadata["gap"] else 0.0,
                unit="boolean",
            )
            yield row


def open_input(path: str) -> IO[str]:
    if path == "-":
        return sys.stdin
    if path.endswith(".gz"):
        return gzip.open(path, mode="rt", encoding="utf-8")
    if path.endswith(".zst"):
        raise ValueError("decompress .zst input to stdin with `zstd -dc` first")
    return open(path, mode="r", encoding="utf-8")


def write_csv(rows: Iterable[dict[str, object]], output: IO[str]) -> None:
    writer = csv.DictWriter(output, fieldnames=FIELDS, extrasaction="ignore")
    writer.writeheader()
    writer.writerows(rows)


def write_parquet(rows: Iterable[dict[str, object]], output_path: str) -> None:
    try:
        import pyarrow as pa
        import pyarrow.parquet as pq
    except ImportError as error:
        raise RuntimeError("Parquet export requires the optional `pyarrow` package") from error

    schema = pa.schema(
        [
            ("timestamp", pa.string()),
            ("seq", pa.int64()),
            ("interval_ms", pa.int64()),
            ("lag_ms", pa.int64()),
            ("missed_intervals", pa.int64()),
            ("gap", pa.bool_()),
            ("hostname", pa.string()),
            ("section", pa.string()),
            ("device_id", pa.string()),
            ("device_name", pa.string()),
            ("metric", pa.string()),
            ("value", pa.float64()),
            ("unit", pa.string()),
        ]
    )
    parquet = pq.ParquetWriter(output_path, schema)
    batch: list[dict[str, object]] = []
    try:
        for row in rows:
            batch.append(row)
            if len(batch) >= 4096:
                parquet.write_table(pa.Table.from_pylist(batch, schema=schema))
                batch.clear()
        if batch:
            parquet.write_table(pa.Table.from_pylist(batch, schema=schema))
    finally:
        parquet.close()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", default="-", help="NDJSON path, .gz path, or stdin (default)")
    parser.add_argument("--format", choices=("csv", "parquet"), default="csv")
    parser.add_argument("--output", default="-", help="output path; CSV defaults to stdout")
    args = parser.parse_args(argv)

    try:
        source = open_input(args.input)
        close_source = source is not sys.stdin
        try:
            rows = iter_rows(source)
            if args.format == "csv":
                if args.output == "-":
                    write_csv(rows, sys.stdout)
                else:
                    with open(args.output, mode="w", newline="", encoding="utf-8") as output:
                        write_csv(rows, output)
            else:
                if args.output == "-":
                    parser.error("--output is required for Parquet")
                write_parquet(rows, args.output)
        finally:
            if close_source:
                source.close()
    except (OSError, ValueError, RuntimeError) as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
