# Copyright (c) 2026 Metrum AI. All rights reserved.
"""Tests for the stdlib CSV path of scripts/record_export.py."""

import io
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
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
        self.assertIn("2026-10-27T12:00:00.123Z,0,250,1,0,False,host,gpus", output.getvalue())
        self.assertTrue(output.getvalue().rstrip().endswith("42.5,percent"))


if __name__ == "__main__":
    unittest.main()
