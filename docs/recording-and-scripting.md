# Snapshots, recording, and replay

[Home](../README.md) · [Documentation](../README.md#documentation)

## Scripting / CI (Snapshot Mode)

The `snapshot` subcommand emits a single, one-shot machine-readable dump of
the current hardware state to stdout (or a file) and exits. It is designed
for shell piping, CI probes, Slurm prolog/epilog hooks, and any tool that
wants `nvidia-smi --query-gpu=... --format=csv` ergonomics without starting
a long-running HTTP server.

```bash
# Default: JSON, using snapshot.default_pretty (true by default).
all-smi snapshot

# Force compact JSON for a pipeline.
all-smi snapshot --format json --pretty=false | jq '.gpus[] | {name, utilization, temperature}'

# CSV with nvidia-smi-style columns. Scope to GPUs with --include gpu to
# keep the rows focused; the default CSV includes one row per CPU/memory
# /chassis device too, which is usually not what you want for GPU tooling.
all-smi snapshot --format csv --include gpu \
  --query uuid,name,utilization,used_memory,total_memory,temperature,power_consumption

# Prometheus exposition matching a single /metrics scrape.
all-smi snapshot --format prometheus > /tmp/snapshot.prom

# Take three samples one second apart, write a JSON array to a file.
all-smi snapshot --samples 3 --interval 1 --output /tmp/snapshot-series.json

# Include opt-in expensive sections (processes, storage).
all-smi snapshot --include gpu,cpu,memory,chassis,process,storage

# Fail if collection or output fails; partial reader errors may still exit 0.
if ! all-smi snapshot --timeout-ms 2000 --format json >/dev/null; then
    echo "snapshot failed"
    exit 1
fi
```

**Exit codes**

| Code | Meaning                                                                   |
|------|---------------------------------------------------------------------------|
| `0`  | Success. Output was written; the `errors` array may contain partial failures. |
| `1`  | Collection or output failure (including no devices collected).                  |
| `2`  | Flag parse error (invalid `--include`, unknown format, etc.).             |

**Temperature-check example** — a starting point for a Slurm prolog, not a complete admission policy. Verify that expected GPUs and temperature readings are present, and handle partial collection errors for your hardware before using it to admit jobs:

```bash
#!/usr/bin/env bash
set -euo pipefail

TEMP_MAX=85
JSON="$(all-smi snapshot --format json --include gpu)"
HOT_COUNT="$(echo "$JSON" | jq "[.gpus[] | select(.temperature >= $TEMP_MAX)] | length")"
if [[ "$HOT_COUNT" -gt 0 ]]; then
    echo "Refusing to start: $HOT_COUNT GPU(s) at or above ${TEMP_MAX}C" >&2
    echo "$JSON" | jq '.gpus[] | {name, temperature}' >&2
    exit 1
fi
```

**CSV → awk pipeline** — compute average utilization across all GPUs:

```bash
all-smi snapshot --format csv --include gpu --query utilization \
  | awk -F, 'NR > 1 { sum += $1; n++ } END { if (n) printf "avg=%.1f%%\n", sum/n }'
```

For the complete option list, run `all-smi snapshot --help`. Partial collection errors appear in JSON's `errors` array or on stderr for CSV/Prometheus. Check errors as well as the exit code before using snapshots as a workload admission gate.

The CLI uses `snapshot.default_pretty` (default `true`), including when piped; use `--pretty=false` for compact output. A configured non-JSON `snapshot.default_format` currently takes precedence even over explicit `--format json`; see [configuration limitations](configuration.md#limitations).

## Recording & Replay

The `record` subcommand captures a live metric stream to disk as NDJSON, and
`view --replay <file>` plays it back through the TUI using the captured data. Intended for post-hoc incident investigation without a
Prometheus retention store — operators can rewind to the moment throughput
cratered and see the captured GPU/CPU/memory/chassis state at that tick.

Each captured data frame retains the JSON snapshot shape and adds recorder
metadata, plus a header frame and sparse index frames every 1000 data frames
to enable fast seeking. The header declares `recording_schema: 2`; the nested
snapshot payload remains schema 1 so existing replay and snapshot readers can
ignore the additional top-level fields. Frame timestamps use RFC3339 UTC with
millisecond precision and identify the scheduled tick (not the completion time
of collection). `seq`, `interval_ms`, `lag_ms`, `missed_intervals`, and `gap`
make the schedule observable; `gap: true` marks a failed or unavailable sample
(source errors are retained per host/section), while `missed_intervals` counts
schedule boundaries crossed even when the current frame contains data.
`lag_ms` reports sub-interval lateness. The header's `units` object declares
units for the standard numeric fields. Rotation writes a header and unit map
for each segment so retained segments remain independently interpretable.

`--interval` accepts integers with optional `ms` or `s` suffixes, has a 100ms
minimum, and does not accept fractional seconds. Bare `--interval 0` retains
the legacy one-second interval. Remote source polling remains limited to whole
seconds even when output frames are scheduled more frequently.

```bash
# Capture 30 seconds of local hardware state to a zstd-compressed file.
all-smi record --output incident.ndjson.zst --duration 30s --interval 1

# Sample at 250 ms for short transients.
all-smi record --output transient.ndjson.zst --duration 10s --interval 250ms

# Record until SIGTERM; rotate at 100MB per segment, keep last 10 files.
all-smi record --output trace.ndjson.zst --max-size 100M --max-files 10

# Record remote cluster scrapes (same HTTP path as `view`).
all-smi record --source remote \
  --hosts http://gpu-node1:9090,http://gpu-node2:9090 \
  --output cluster.ndjson.gz --compress gzip --duration 1h

# Replay a captured file — identical TUI to the live view.
all-smi view --replay incident.ndjson.zst

# Start playback 14 minutes 32 seconds into the recording and loop.
all-smi view --replay incident.ndjson.zst --start 00:14:32 --loop

# Replay at 4x speed.
all-smi view --replay incident.ndjson.zst --speed 4.0
```

**Replay-mode keybindings** (active only with `--replay`):

| Key          | Action                                               |
|--------------|------------------------------------------------------|
| `SPACE`      | Play / pause                                         |
| `]` / `[`    | Step one frame forward / back (auto-pauses)          |
| `+` / `-`    | Cycle speed through `0.25, 0.5, 1.0, 2.0, 4.0, 8.0`  |
| `j` / `k`    | Seek -10s / +10s                                     |
| `g`          | Open timecode editor (`HH:MM:SS` then Enter)         |
| `L`          | Toggle loop playback                                 |
| `q` / `Esc`  | Quit (`Esc` closes active panels/filters first)                                          |

The status bar shows `REPLAY | HH:MM:SS | frame N / M | Xx | playing/paused`.
Filter-edit mode (`/`) still takes precedence over replay keys, so the
operator can filter the visible GPUs mid-playback.

For the complete option list, run `all-smi record --help` and `all-smi view --help`. Compression follows `--compress`, then `record.compress` (default `zstd`), not just the output suffix. Match the suffix to the selected codec because replay detects compression from the filename. Rotation limits count uncompressed bytes.

### Exporting a recorded time series

`scripts/record_export.py` converts the recording stream to normalized long-form
CSV with timestamps, sequence/cadence fields, gap status, device identity,
metric name, value, and unit. Valid measurements from responding remote hosts
have `gap=false`; an unsuccessful endpoint gets its own named `recording/gap`
row, so a partial scrape does not mark other hosts' measurements as gaps. The
exporter omits unavailable GPU placeholders and memory fields that the
Prometheus exporter omits (zero buffers/cache and swap metrics when swap total
is zero). Plain NDJSON and gzip files can be passed directly; for zstd,
decompress to stdin:

```bash
python3 scripts/record_export.py --input trace.ndjson --format csv --output trace.csv
python3 scripts/record_export.py --input trace.ndjson.gz --format csv --output trace.csv
zstd -dc trace.ndjson.zst | python3 scripts/record_export.py --format csv --output trace.csv
```

The same converter writes Parquet when the optional Python `pyarrow` package is
installed. It streams batches instead of loading the complete trace into memory:

```bash
zstd -dc trace.ndjson.zst | python3 scripts/record_export.py \
  --format parquet --output trace.parquet
```

CSV uses only the Python standard library. Parquet is optional and does not add
a runtime dependency to all-smi.

The default output is `<cache>/all-smi/records/all-smi-record.ndjson.zst`; override it with `record.output_dir` or `--output`. See [cache paths](configuration.md#cache-paths). Local recording and snapshots use the snapshot collection path; available fields can differ from live local mode. Replay cannot reconstruct fields that were not captured.

**Security invariants**

`record` and `view --replay` apply the following hardening that cannot be
disabled:

- **Writer — symlink refusal.** On Unix the output file is opened with
  `O_NOFOLLOW` and mode `0600`. A pre-existing symlink at the target path
  causes `record` to exit immediately rather than follow the link. The same
  check applies when rolling over to a numbered segment: if a symlink is found
  at the rollover path, the rotation is aborted.
- **Replay — per-line size cap.** Each decompressed NDJSON line is limited to
  16 MiB. A `.zst` or `.gz` file that expands a single line beyond this limit
  (decompression-bomb attack) is treated as a corrupted line: it is skipped
  with a warning and reading continues from the next line.
- **Replay — zstd window ceiling.** The zstd decoder is configured with a
  128 MiB window-log ceiling. A stream declaring a larger window is rejected
  before decompression begins.
- **Replay — hosts list cap.** A replay header advertising more than 1024 host
  entries is truncated to 1024 at parse time so the TUI tab row cannot be
  overwhelmed by a hostile recording.
