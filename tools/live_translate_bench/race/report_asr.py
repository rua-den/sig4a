"""Summarize ASR-only diagnostics on the exact synthetic fixture schedule."""
import argparse
import json
import math
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from run_benchmark import load, rows, verify_provenance


def words(text):
    return re.findall(r"[a-z0-9]+", text.lower())


def distance(expected, observed):
    row = list(range(len(observed) + 1))
    for index, word in enumerate(expected, 1):
        next_row = [index]
        for j, found in enumerate(observed, 1):
            next_row.append(min(next_row[-1] + 1, row[j] + 1, row[j - 1] + (found != word)))
        row = next_row
    return row[-1]


def report(manifest, measured):
    by_id = {row["id"]: row for row in measured}
    n = covered = first = silence_violations = unavailable_silence = refs = edits = 0
    lag = []
    for entry in manifest["entries"]:
        row = by_id.get(entry["id"], {})
        hypothesis = words(row.get("transcript") or "")
        if entry["kind"] == "silence":
            unavailable_silence += not row or "error" in row
            silence_violations += bool(hypothesis)
            continue
        n += 1
        reference = words(entry["text"])
        first += bool(hypothesis and hypothesis[0] == reference[0])
        refs += len(reference)
        edits += distance(reference, hypothesis)
        valid = ("error" not in row and hypothesis and row.get("recognition_final_ms") is not None
                 and row["recognition_final_ms"] >= row["input_ready_ms"])
        covered += bool(valid)
        lag.append(row["recognition_final_ms"] - row["speech_end_ms"] if valid else math.inf)
    lag.sort()

    def rank(p):
        value = lag[math.ceil(len(lag) * p) - 1]
        return value if math.isfinite(value) else None

    return {"diagnostic_only": True, "oracle_segment_boundaries": True,
            "utterances": n, "nonempty_timed_recognitions": covered, "first_word_exact": first,
            "words_ref_total": refs, "word_edit_distance": edits, "naive_word_error_rate": edits / refs,
            "asr_final_from_speech_end_ms": {"p50": rank(.5), "p95": rank(.95)},
            "silence_word_violations": silence_violations, "unavailable_silence_controls": unavailable_silence,
            "limits": "No VAD or live audio capture. Naive WER penalizes textual number formatting. No translation/provisional phase gate."}


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--results", required=True, type=Path)
    ap.add_argument("--frozen", required=True, type=Path)
    args = ap.parse_args()
    manifest, measured = load(), rows(args.results)
    verify_provenance(manifest, args.results, args.frozen, measured)
    print(json.dumps(report(manifest, measured), indent=2))
