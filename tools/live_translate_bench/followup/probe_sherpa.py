"""Diagnose leading-word loss on known synthetic WAV, without translating or changing ASR."""
from __future__ import annotations

import argparse
import array
import json
from pathlib import Path
import re
import sys
import time
from collections import Counter

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
from measure import read_pcm
from run_benchmark import MANIFEST, RATE, GAP_MS, digest, load, validate

CHUNK = RATE // 10
VARIANTS = [
    ("one_shot", None),
    ("paced_0ms", 0),
    ("paced_500ms", 500),
    ("paced_800ms", 800),
]


def decode_probe(args):
    from sherpa_adapter import Sherpa
    m = load()
    if errors := validate(m):
        raise ValueError(errors)
    frozen = json.loads(args.frozen.read_bytes())
    if frozen["fixture_sha256"] != digest(MANIFEST):
        raise ValueError("manifest hash differs from frozen synthetic WAV")
    model = Sherpa(args.model_dir, 4)
    inputs = []
    for row in m["entries"]:
        if row["kind"] != "utterance":
            continue
        source = args.audio / f"{row['id']}.wav"
        audio = read_pcm(source)
        meta = frozen["frames"][row["id"]]
        if digest(source) != meta["sha256"]:
            raise ValueError("synthetic WAV content differs from frozen input")
        samples = array.array("h", audio)
        if sys.byteorder != "little":
            samples.byteswap()
        end = meta["speech_end"]
        if not 0 < end <= len(samples):
            raise ValueError("invalid speech end")
        begin = next(i for i, s in enumerate(samples[:end]) if abs(s) >= 64)
        baseline = audio[:2 * end] + b"\0" * (RATE * GAP_MS // 1000 * 2)
        inputs.append((row, baseline, begin, end))
    warm = read_pcm(args.audio / "WARM.wav")
    if digest(args.audio / "WARM.wav") != frozen["frames"]["WARM"]["sha256"] or not model.recognize(warm):
        raise ValueError("wrong source or failed model warmup")

    starts = Counter()
    summary = Counter()
    with args.output.open("x", encoding="utf8") as fp:
        for name, pad_ms in VARIANTS:
            for row, pcm, begin, end in inputs:
                lead = RATE * (pad_ms or 0) // 1000
                pcm = b"\0" * (2 * lead) + pcm
                frame_times = []
                first_partial_ms = None
                stream = model.stream()
                anchor = time.perf_counter()

                def now_ms():
                    return round(1000 * (time.perf_counter() - anchor), 2)

                if pad_ms is None:
                    model.advance(stream, pcm)
                else:
                    for start in range(0, len(pcm), 2 * CHUNK):
                        end_byte = min(len(pcm), start + 2 * CHUNK)
                        nominal = 1000 * end_byte / (RATE * 2)
                        delay = (nominal - now_ms()) / 1000
                        if delay > 0:
                            time.sleep(delay)
                        arrival = now_ms()
                        model.advance(stream, pcm[start:end_byte])
                        frame_times.append({"scheduled_ms": round(nominal, 2), "arrived_ms": arrival,
                                            "decode_returned_ms": now_ms()})
                        if first_partial_ms is None and model.recognizer.get_result(stream).strip():
                            first_partial_ms = now_ms()
                transcript = model.finish(stream)
                runtime_ms = now_ms()
                reference_words = re.findall(r"[A-Za-z0-9]+", row["text"].lower())
                transcript_words = re.findall(r"[A-Za-z0-9]+", transcript.lower())
                first_ok = bool(transcript_words and transcript_words[0] == reference_words[0])
                starts[name] += first_ok
                summary[name] += 1
                out = {"id": row["id"], "variant": name, "reference": row["text"], "transcript": transcript,
                       "leading_padding_ms": pad_ms, "first_word_exact_casefolded": first_ok,
                       "first_partial_ms": first_partial_ms, "decode_finished_ms": runtime_ms,
                       "first_active_sample_ms": (begin + lead) * 1000 / RATE,
                       "last_active_sample_boundary_ms": (end + lead) * 1000 / RATE,
                       "scheduled_frame_times": frame_times}
                fp.write(json.dumps(out, ensure_ascii=False) + "\n")
                fp.flush()
    args.output.with_suffix(".summary.json").write_text(json.dumps({
        "diagnostic_only": True, "variants": {name: {"first_word_matches": starts[name], "total": count}
                                                for name, count in summary.items()},
        "same_frozen_audio_sha256": digest(args.frozen), "model_version": model.version,
        "model_weights_sha256": {key: digest(value) for key, value in model.files.items()},
        "output_sha256": digest(args.output),
        "limits": "Synthetic voice recipe; first casefolded alphanumeric word checks prefixes only; no independent human or physical evidence.",
    }, indent=2), encoding="utf8")


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    for flag in ("audio", "frozen", "model-dir", "output"):
        ap.add_argument("--" + flag, type=Path, required=True)
    decode_probe(ap.parse_args())
