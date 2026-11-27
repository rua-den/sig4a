"""Validate frozen synthetic fixture definitions, or score a single measured replay."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path

GAP_MS = 700
RATE = 16000
MANIFEST = Path(__file__).resolve().with_name("fixture-manifest.json")


def digest(path):
    with path.open("rb") as reader:
        return hashlib.file_digest(reader, "sha256").hexdigest()


def verify_provenance(manifest, output_path, frozen_path, result_rows):
    sidecar = json.loads(frozen_path.read_bytes())
    metadata = json.loads(output_path.with_suffix(".metadata.json").read_bytes())
    if not (digest(MANIFEST) == sidecar["fixture_sha256"] == metadata["fixture_sha256"]
            and digest(frozen_path) == metadata["frozen_sha256"]
            and digest(output_path) == metadata["result_sha256"]):
        raise ValueError("manifest, frozen sidecar or captions do not match recorded SHA-256")
    marked = index(result_rows, {row["id"] for row in manifest["entries"]})
    position = 0
    for entry in manifest["entries"]:
        row = marked.get(entry["id"])
        if entry["kind"] == "utterance":
            end_frame = sidecar["frames"][entry["id"]]["speech_end"]
            speech_end = round((position + end_frame) * 1000 / RATE)
            position += end_frame + RATE * GAP_MS // 1000
        else:
            speech_end = round(position * 1000 / RATE)
            position += RATE * entry["duration_ms"] // 1000
        if row is None:
            continue
        ready = round(position * 1000 / RATE)
        if row["engine"] != metadata["asr_engine"] or row.get("speech_end_ms") != speech_end or row.get("input_ready_ms") != ready:
            raise ValueError("row engine or recorded schedule contradicts frozen fixture")
        if row.get("visible_final_caption_ms") is not None and not (
            ready <= row["recognition_final_ms"] <= row["translation_final_ms"] <= row["visible_final_caption_ms"]
        ):
            raise ValueError("caption timing stages are out of order")


def load():
    return json.loads(MANIFEST.read_text(encoding="utf-8"))


def validate(m):
    entries = m.get("entries", [])
    ids = [row.get("id") for row in entries]
    errors = []
    if len(ids) != len(set(ids)):
        errors.append("duplicate entry IDs")
    utterances = [row for row in entries if row.get("kind") == "utterance"]
    silence = [row for row in entries if row.get("kind") == "silence"]
    if len(utterances) < 40 or len(silence) < 2 or len(entries) != len(utterances) + len(silence):
        errors.append("need at least 40 utterances, two silence controls, and no unknown kinds")
    for row in entries:
        name = row.get("id")
        if not isinstance(name, str) or len(name) != 4 or name[0] not in "US" or not name[1:].isascii() or not name[1:].isdigit():
            errors.append("invalid row ID")
        if row.get("source") != "synthetic":
            errors.append(f"{name}: only synthetic entries are allowed")
        if row.get("kind") == "utterance" and (not row.get("text") or not row.get("facts")):
            errors.append(f"{name}: missing text or reference facts")
        if row.get("kind") == "silence" and not 0 < row.get("duration_ms", 0) <= 15000:
            errors.append(f"{name}: invalid duration")
    if m.get("scoring", {}).get("missing_counts_as_infinity") is not True:
        errors.append("missing_counts_as_infinity must be true")
    return errors


def index(rows, allowed):
    result = {}
    for row in rows:
        identity = row["id"]
        if identity not in allowed or identity in result:
            raise ValueError(f"duplicate or unknown row ID: {identity}")
        result[identity] = row
    return result


def score(m, rows, annotations):
    allowed = {row["id"] for row in m["entries"]}
    actual = index(rows, allowed)
    manual = index(annotations, allowed)
    if len({row["engine"] for row in actual.values()}) > 1:
        raise ValueError("score one engine/measurement run at a time")
    deadlines = []
    missing_silence = silence_violations = matched = facts = 0
    missing_reviews = reversals = unreviewed_negations = 0
    for row in m["entries"]:
        r = actual.get(row["id"], {})
        painted = r.get("visible_final_caption_ms")
        if row["kind"] == "silence":
            missing_silence += not r or bool(r.get("error"))
            silence_violations += painted is not None
            continue
        facts += len(row["facts"])
        if painted is None:
            deadlines.append(math.inf)
            unreviewed_negations += row["critical_negation"]
            continue
        spoken = r["speech_end_ms"]
        if type(spoken) is not int or type(painted) is not int or not 0 <= spoken <= painted or not r.get("translation"):
            raise ValueError("Caption timestamps must be ordered nonnegative integers with actual translated text.")
        deadlines.append(painted - spoken)
        review = manual.get(row["id"])
        if review is None:
            missing_reviews += 1
            unreviewed_negations += row["critical_negation"]
            continue
        matches = review["facts_matched"]
        if not isinstance(matches, list) or not set(matches) <= set(row["facts"]):
            raise ValueError("review must use the predeclared reference-fact labels")
        matched += len(set(matches))
        if row["critical_negation"]:
            if review.get("critical_negation_ok") is False:
                reversals += 1
            elif review.get("critical_negation_ok") is not True:
                unreviewed_negations += 1
    ordered = sorted(deadlines)

    def nearest_rank(p):
        result = ordered[math.ceil(p * len(ordered)) - 1]
        return result if math.isfinite(result) else None

    missing = sum(not math.isfinite(x) for x in ordered)
    p50, p95 = nearest_rank(.50), nearest_rank(.95)
    return {
        "utterances": len(ordered), "coverage": 1 - missing / len(ordered),
        "missing_captions_counted_as_infinity": missing,
        "latency_nearest_rank_ms": {"p50": p50, "p95": p95},
        "fact_recall_conservative": matched / facts, "facts_matched": matched,
        "facts_total_including_missing_captions": facts,
        "missing_caption_reviews": missing_reviews, "critical_negation_reversals": reversals,
        "unreviewed_critical_negations": unreviewed_negations,
        "silence_caption_violations": silence_violations, "unmeasured_silence_controls": missing_silence,
        "passes_provisional_numeric_targets": p50 is not None and p50 <= 2500 and p95 is not None
        and p95 <= 5000 and matched / facts >= .9 and not any(
            (missing_reviews, reversals, unreviewed_negations, silence_violations, missing_silence)
        ),
    }


def rows(path):
    return [json.loads(line) for line in Path(path).read_text(encoding="utf-8-sig").splitlines() if line.strip()]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--validate-manifest", action="store_true")
    parser.add_argument("--results")
    parser.add_argument("--frozen", type=Path, help="frozen input sidecar required for evidence grading")
    parser.add_argument("--annotations", help="separate manual JSONL; never use model/ASR output as gold")
    args = parser.parse_args()
    m = load()
    if errors := validate(m):
        print(json.dumps({"valid": False, "errors": errors}, indent=2))
        return 1
    if args.validate_manifest or not args.results:
        result = {"valid": True, "utterances": sum(r["kind"] == "utterance" for r in m["entries"]),
                  "silence_controls": sum(r["kind"] == "silence" for r in m["entries"]),
                  "manifest_sha256": hashlib.sha256(MANIFEST.read_bytes()).hexdigest()}
    else:
        measured = rows(args.results)
        if args.annotations and args.frozen is None:
            parser.error("--annotations requires --frozen and matching run metadata")
        if args.frozen is not None:
            verify_provenance(m, Path(args.results), args.frozen, measured)
        result = score(m, measured, rows(args.annotations) if args.annotations else [])
        result["provenance_verified"] = args.frozen is not None
    print(json.dumps(result, indent=2, allow_nan=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
