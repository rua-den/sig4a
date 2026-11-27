"""Mechanism self-tests only; synthetic scorer rows are never run evidence."""
import argparse
import json
from pathlib import Path
import tempfile
import unittest

from continuous import RATE, digest, score, write_json


class ScoresEndpointEvidence(unittest.TestCase):
    def test_preserves_duplicate_and_silence_events_and_missing_latency(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp)
            (out / "tape.wav").write_bytes(b"deterministic dry fixture")
            hashed_tape = digest(out / "tape.wav")
            labels = [
                {"id": "U001", "kind": "utterance", "associated_from_frame": 0,
                 "associated_until_frame": RATE * 2, "speech_end_frame": RATE},
                {"id": "U002", "kind": "utterance", "associated_from_frame": RATE * 2,
                 "associated_until_frame": RATE * 4, "speech_end_frame": RATE * 3},
                {"id": "S001", "kind": "silence", "associated_from_frame": RATE * 4,
                 "associated_until_frame": RATE * 6, "speech_end_frame": RATE * 5},
            ]
            events = [
                {"endpoint_frame": 24000, "visible_final_caption_ms": 2000, "translation": "Mẫu một"},
                {"endpoint_frame": 53000, "visible_final_caption_ms": 4000, "translation": "Mẫu hai"},
                {"endpoint_frame": 59000, "visible_final_caption_ms": 4200, "translation": "Mẫu hai lần nữa"},
                {"endpoint_frame": 84000, "visible_final_caption_ms": None, "transcript": "spurious"},
            ]
            write_json(out / "scoring-only-boundaries.json", {"tape_sha256": hashed_tape, "rows": labels})
            path = out / "dynamic-events.jsonl"
            path.write_text("".join(json.dumps(row) + "\n" for row in events), encoding="utf-8")
            write_json(out / "run-provenance.json", {"tape_sha256": hashed_tape, "event_sha256": digest(path)})
            score(argparse.Namespace(output=out))
            data = json.loads((out / "timing-diagnostic.json").read_bytes())
            self.assertEqual(data["original_utterances"], 2)
            self.assertEqual(data["single_visible_translations"], 1)
            self.assertEqual(data["noise_or_silence_endpoints"], 1)
            self.assertEqual(data["p50_ms"], 1000)
            self.assertIsNone(data["p95_ms"])
            path.write_text("tampered", encoding="utf-8")
            with self.assertRaises(ValueError):
                score(argparse.Namespace(output=out))


if __name__ == "__main__":
    unittest.main()
