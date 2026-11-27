"""Diagnostic only: compare offline MT on clean and cached synthetic ASR inputs."""
from __future__ import annotations

import argparse
from collections import defaultdict
import hashlib
import importlib.metadata
import json
import math
import os
from pathlib import Path
import platform
import sys
import time

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
from measure import translate
from run_benchmark import MANIFEST, digest, load, validate

OPUS_REV = "3ab3b2d9aa096a3b0c20668d87566bb92613aba1"
OPUS_WEIGHT_HASH = "652c300995d3d074c558ed799dbc9afba223441102cafee1b20e1c52a825d3a6"
LFM_WEIGHT_HASH = "263aca93039e22140d55e046831c700c796affa8143d7638581c488a30c712bc"
PROJECTION = HERE / "exact26-synthetic-projection.json"
EXPECTED = {"config.json", "tokenizer_config.json", "source.spm", "target.spm", "vocab.json", "model.safetensors"}


def corpus():
    manifest = load()
    if errors := validate(manifest):
        raise ValueError(errors)
    cached = json.loads(PROJECTION.read_bytes())
    if cached["fixture_lf_sha256"] != hashlib.sha256(MANIFEST.read_bytes().replace(b"\r\n", b"\n")).hexdigest():
        raise ValueError("cached ASR must match exact benchmark fixture")
    reference = {row["id"]: row["text"] for row in manifest["entries"] if row["kind"] == "utterance"}
    expected = {(name, source) for name in reference for source in ("whisper", "sherpa")}
    observed = {(row["id"], row["source"]) for row in cached["rows"]}
    if observed != expected or len(cached["rows"]) != len(observed):
        raise ValueError("cached ASR set is incomplete or contains duplicates")
    return reference, cached


def comparison(model_dir, output, port, lfm_path):
    if {p.name for p in model_dir.iterdir() if p.is_file()} != EXPECTED:
        raise ValueError("only allowlisted local Marian data files may be present")
    if digest(model_dir / "model.safetensors") != OPUS_WEIGHT_HASH or digest(lfm_path) != LFM_WEIGHT_HASH:
        raise ValueError("unrecognized translator weights")
    if output.exists():
        raise FileExistsError(output)
    original, cached = corpus()
    os.environ.update(HF_HUB_OFFLINE="1", TRANSFORMERS_OFFLINE="1", HF_HUB_DISABLE_TELEMETRY="1")
    import torch
    from transformers import MarianMTModel, MarianTokenizer

    torch.set_num_threads(4)
    torch.set_num_interop_threads(1)
    start = time.perf_counter()
    tokenizer = MarianTokenizer.from_pretrained(str(model_dir), local_files_only=True)
    model = MarianMTModel.from_pretrained(str(model_dir), local_files_only=True,
                                            use_safetensors=True).eval()
    opus_load_ms = round(1000 * (time.perf_counter() - start), 1)

    def opus(text):
        with torch.inference_mode():
            batch = tokenizer(text, return_tensors="pt", truncation=True, max_length=256)
            result = model.generate(**batch, num_beams=4, do_sample=False, max_new_tokens=120)
        return tokenizer.decode(result[0], skip_special_tokens=True).strip() or None

    warmup = "This sentence only warms the synthetic test."
    if not opus(warmup) or not translate(port, warmup, "llama.cpp")[0]:
        raise RuntimeError("translator warmup yielded no text")

    inputs = [(name, "reference", text, None) for name, text in original.items()]
    inputs += [(row["id"], row["source"], row["asr_text"], row) for row in cached["rows"]]
    lag = defaultdict(list)
    columns = defaultdict(lambda: [0, 0])
    with output.open("x", encoding="utf-8") as fp:
        for name, source, text, row in inputs:
            when = time.perf_counter()
            mt_opus = opus(text) if text else None
            opus_ms = round(1000 * (time.perf_counter() - when), 1) if text else None
            if row:
                mt_lfm, lfm_ms = row["lfm_cached"], row["prior_mt_span_ms"]
                lfm_origin = "exact26_cached_prior_translation_span_not_same-run"
            else:
                when = time.perf_counter()
                mt_lfm = translate(port, text, "llama.cpp")[0]
                lfm_ms = round(1000 * (time.perf_counter() - when), 1) if text else None
                lfm_origin = "followup_fresh"
            for engine, value, elapsed in [("opus", mt_opus, opus_ms), ("lfm", mt_lfm, lfm_ms)]:
                key = f"{engine}:{source}"
                columns[key][0] += bool(value)
                columns[key][1] += 1
                if elapsed is not None and (engine != "lfm" or not row):
                    lag[key].append(elapsed)
            fp.write(json.dumps({"id": name, "source": source, "input": text,
                                 "opus": mt_opus, "opus_ms": opus_ms, "lfm": mt_lfm,
                                 "lfm_ms": lfm_ms, "lfm_origin": lfm_origin}, ensure_ascii=False) + "\n")
            fp.flush()

    def rank(values, percentile):
        return sorted(values)[math.ceil(len(values) * percentile) - 1] if values else None

    summary = {
        "diagnostic_only": True,
        "machine": platform.platform(),
        "dependencies": {p: importlib.metadata.version(p) for p in ("transformers", "torch", "safetensors", "sentencepiece")},
        "opus_revision": OPUS_REV, "opus_model_load_ms": opus_load_ms,
        "model_files": {name: {"sha256": digest(model_dir / name), "bytes": (model_dir / name).stat().st_size}
                        for name in sorted(EXPECTED)},
        "lfm_weights_sha256": LFM_WEIGHT_HASH,
        "fixture_sha256": digest(MANIFEST), "cached_projection_sha256": digest(PROJECTION),
        "results_sha256": digest(output),
        "rows": {name: {"nonempty": n, "total": total} for name, (n, total) in columns.items()},
        "fresh_mt_latency_ms": {name: {"p50": rank(values, .5), "p95": rank(values, .95)}
                                for name, values in lag.items()},
        "accuracy": "UNSCORED: separate bilingual adjudication required; nonempty is not correct",
    }
    output.with_suffix(".summary.json").write_text(json.dumps(summary, indent=2), encoding="utf8")


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check-input", action="store_true")
    ap.add_argument("--model-dir", type=Path)
    ap.add_argument("--lfm-weights", type=Path)
    ap.add_argument("--lfm-port", type=int, default=18081)
    ap.add_argument("--output", type=Path)
    a = ap.parse_args()
    if a.check_input:
        reference, cached = corpus()
        print(f"verified {len(reference)} clean references, {len(cached['rows'])} cached ASR inputs")
    elif a.model_dir and a.lfm_weights and a.output:
        comparison(a.model_dir, a.output, a.lfm_port, a.lfm_weights)
    else:
        ap.error("--model-dir, --lfm-weights and --output required")
