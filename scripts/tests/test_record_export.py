# Copyright 2026 Metrum AI
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

"""Tests for the stdlib CSV path of scripts/record_export.py."""

import gzip
import importlib.util
import io
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import record_export


class RecordExportTests(unittest.TestCase):
    def test_flattens_numeric_metrics_with_header_units_and_timing(self) -> None:
        stream = io.StringIO(
            '{"header":true,"recording_schema":2,"units":{"gpus.utilization":"percent"}}\n'
            '{"timestamp":"2026-10-27T12:00:00.123Z","hostname":"host",'
            '"seq":3,"interval_ms":250,"lag_ms":4,"missed_intervals":0,"gap":false,'
            '"gpus":[{"uuid":"gpu-0","name":"GPU","utilization":42.5,"used_memory":1024}]}\n'
        )
        rows = list(record_export.iter_rows(stream))

        self.assertEqual([row["metric"] for row in rows], ["utilization", "used_memory"])
        self.assertEqual(rows[0]["unit"], "percent")
        self.assertEqual(rows[0]["value"], 42.5)
        self.assertEqual(rows[1]["unit"], "")
        self.assertEqual(rows[0]["seq"], 3)
        self.assertEqual(rows[0]["lag_ms"], 4)
        self.assertFalse(rows[0]["gap"])

    def test_gap_frame_remains_visible_without_device_metrics(self) -> None:
        rows = list(
            record_export.iter_rows(
                ['{"timestamp":"2026-10-27T12:00:00.000Z","gap":true,"errors":[]}']
            )
        )

        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["section"], "recording")
        self.assertEqual(rows[0]["metric"], "gap")
        self.assertTrue(rows[0]["gap"])
        self.assertEqual(rows[0]["unit"], "boolean")

    def test_unavailable_gpu_placeholders_are_omitted_and_host_is_device_scoped(self) -> None:
        stream = io.StringIO(
            '{"hostname":"remote","gpus":[{"uuid":"gpu-0","hostname":"node-a",'
            '"utilization":-1.0,"ane_utilization":-1.0,"power_consumption":-1.0,'
            '"temperature":0,"frequency":0,"used_memory":0}]}'
        )
        rows = list(record_export.iter_rows(stream))
        self.assertEqual([row["metric"] for row in rows], ["used_memory"])
        self.assertEqual(rows[0]["hostname"], "node-a")
        self.assertFalse(rows[0]["gap"])

    def test_partial_remote_gap_is_host_scoped_and_memory_placeholders_are_filtered(self) -> None:
        stream = io.StringIO(
            '{"hostname":"remote","gap":true,"errors":[{"section":"remote host node-b:9090",'
            '"kind":"timeout","message":"no response"}],'
            '"gpus":[{"uuid":"gpu-0","hostname":"node-a","name":"GPU", "utilization":42}],'
            '"memory":[{"hostname":"node-a","total_bytes":0,"used_bytes":0,'
            '"swap_total_bytes":0,"swap_used_bytes":0,"swap_free_bytes":0,'
            '"buffers_bytes":0,"cached_bytes":0}]}'
        )
        rows = list(record_export.iter_rows(stream))

        gpu_rows = [row for row in rows if row["section"] == "gpus"]
        self.assertEqual(len(gpu_rows), 1)
        self.assertFalse(gpu_rows[0]["gap"])
        self.assertEqual(gpu_rows[0]["hostname"], "node-a")
        memory_metrics = {row["metric"] for row in rows if row["section"] == "memory"}
        self.assertEqual(memory_metrics, {"total_bytes", "used_bytes"})
        gap_rows = [row for row in rows if row["metric"] == "gap"]
        self.assertEqual(len(gap_rows), 1)
        self.assertEqual(gap_rows[0]["hostname"], "node-b:9090")
        self.assertEqual(gap_rows[0]["device_id"], "node-b:9090")
        self.assertTrue(gap_rows[0]["gap"])

    def test_rotated_segment_header_supplies_its_unit_map(self) -> None:
        stream = io.StringIO(
            '{"header":true,"units":{"memory.used_bytes":"bytes"}}\n'
            '{"memory":[{"index":0,"used_bytes":5}]}'
        )
        rows = list(record_export.iter_rows(stream))
        self.assertEqual(rows[0]["unit"], "bytes")
        self.assertEqual(rows[0]["value"], 5.0)

    def test_gzip_input_can_be_exported(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "recording.ndjson.gz"
            with gzip.open(path, "wt", encoding="utf-8") as compressed:
                compressed.write('{"hostname":"host","memory":[{"index":0,"hostname":"node-a","used_bytes":3}]}\n')
            with record_export.open_input(str(path)) as stream:
                rows = list(record_export.iter_rows(stream))
        self.assertEqual(rows[0]["hostname"], "node-a")
        self.assertEqual(rows[0]["metric"], "used_bytes")

    @unittest.skipUnless(importlib.util.find_spec("pyarrow"), "pyarrow is optional")
    def test_parquet_writer_emits_rows_when_pyarrow_is_available(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "recording.parquet")
            record_export.write_parquet(iter([{"timestamp": "t", "seq": 0, "interval_ms": 1,
                "lag_ms": 0, "missed_intervals": 0, "gap": None, "hostname": "host",
                "section": "gpus", "device_id": "gpu-0", "device_name": "GPU",
                "metric": "used_memory", "value": 1.0, "unit": "bytes"}]), path)
            import pyarrow.parquet as pq
            self.assertEqual(pq.read_table(path).num_rows, 1)

    def test_csv_writer_emits_timestamp_and_unit_columns(self) -> None:
        output = io.StringIO()
        record_export.write_csv(
            iter(
                [
                    {
                        "timestamp": "2026-10-27T12:00:00.123Z",
                        "seq": 0,
                        "interval_ms": 250,
                        "lag_ms": 1,
                        "missed_intervals": 0,
                        "gap": False,
                        "hostname": "host",
                        "section": "gpus",
                        "device_id": "gpu-0",
                        "device_name": "GPU",
                        "metric": "utilization",
                        "value": 42.5,
                        "unit": "percent",
                    }
                ]
            ),
            output,
        )
        self.assertIn("timestamp,seq,interval_ms,lag_ms", output.getvalue())
        self.assertIn("2026-10-27T12:00:00.123Z,0,250,1,0,false,host,gpus", output.getvalue())
        self.assertTrue(output.getvalue().rstrip().endswith("42.5,percent"))


if __name__ == "__main__":
    unittest.main()
