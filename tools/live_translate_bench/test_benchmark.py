import json
from pathlib import Path
import tempfile
import unittest

from run_benchmark import MANIFEST, digest, load, score, validate, verify_provenance


class BenchmarkScoring(unittest.TestCase):
    def setUp(self):
        self.fixture = load()
        self.entries = self.fixture["entries"]
        self.rows = []
        self.reviews = []
        for entry in self.entries:
            row = {"id": entry["id"], "engine": "unit-fixture", "speech_end_ms": 2000,
                   "visible_final_caption_ms": None}
            if entry["kind"] == "utterance":
                row.update(visible_final_caption_ms=3000, translation="Mẫu")
                self.reviews.append({"id": entry["id"], "facts_matched": entry["facts"],
                                     "critical_negation_ok": True})
            self.rows.append(row)

    def test_clean_and_ungraded(self):
        self.assertEqual(validate(self.fixture), [])
        self.assertTrue(score(self.fixture, self.rows, self.reviews)["passes_provisional_numeric_targets"])
        self.assertFalse(score(self.fixture, self.rows, [])["passes_provisional_numeric_targets"])

    def test_missing_critical_blocks_even_if_p95_still_finite(self):
        report = score(self.fixture, [row for row in self.rows if row["id"] != "U003"], self.reviews)
        self.assertEqual(report["latency_nearest_rank_ms"]["p95"], 1000)
        self.assertEqual(report["unreviewed_critical_negations"], 1)
        self.assertFalse(report["passes_provisional_numeric_targets"])

    def test_missing_reports_json_null_and_retains_facts(self):
        empty = score(self.fixture, [], [])
        self.assertIsNone(empty["latency_nearest_rank_ms"]["p95"])
        self.assertEqual(empty["facts_total_including_missing_captions"], 122)
        json.dumps(empty, allow_nan=False)

    def test_duplicate_and_impossible_timestamp(self):
        with self.assertRaises(ValueError):
            score(self.fixture, self.rows + self.rows[:1], self.reviews)
        self.rows[0]["visible_final_caption_ms"] = 1999
        with self.assertRaises(ValueError):
            score(self.fixture, self.rows, self.reviews)

    def test_sidecar_binding(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            frozen, output = base / "frozen.json", base / "out.jsonl"
            sidecar = {"fixture_sha256": digest(MANIFEST), "frames": {
                entry["id"]: {"speech_end": 16000} for entry in self.entries if entry["kind"] == "utterance"
            }}
            frozen.write_text(json.dumps(sidecar))
            row = {"id": "U001", "engine": "unit-fixture", "speech_end_ms": 1000, "input_ready_ms": 1700,
                   "recognition_final_ms": 1900, "translation_final_ms": 2000, "visible_final_caption_ms": 2008}
            output.write_text(json.dumps(row))
            output.with_suffix(".metadata.json").write_text(json.dumps({
                "asr_engine": "unit-fixture", "fixture_sha256": digest(MANIFEST),
                "frozen_sha256": digest(frozen), "result_sha256": digest(output),
            }))
            verify_provenance(self.fixture, output, frozen, [row])
            with self.assertRaises(ValueError):
                verify_provenance(self.fixture, output, frozen, [{**row, "speech_end_ms": 42}])
            output.write_text("tampered")
            with self.assertRaises(ValueError):
                verify_provenance(self.fixture, output, frozen, [row])


if __name__ == "__main__":
    unittest.main()
