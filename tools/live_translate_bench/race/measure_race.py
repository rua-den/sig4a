"""Opt-in synthetic race on exact #27 audio using alternate local ASR + Marian."""
from __future__ import annotations

import argparse
from importlib.metadata import version
import os
from pathlib import Path
import sys
import time

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parent), str(HERE.parent / "followup")]

from compare_mt import EXPECTED, OPUS_REV, OPUS_WEIGHT_HASH
from measure import run
from run_benchmark import digest
from sherpa_adapter import Sherpa

FROZEN = "68fd06d26e24e8f0e25f93e068261e01c67436c7c5dc6da256e2521a7a42f288"
TINY_EN = "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f"


class LargerSherpa(Sherpa):
    engine_id = "sherpa-onnx-zipformer-en-2023-06-26-int8"

    def __init__(self, model_dir, threads):
        import sherpa_onnx
        stem = "epoch-99-avg-1-chunk-16-left-128"
        self.files = {
            "tokens": model_dir / "tokens.txt",
            "encoder": model_dir / f"encoder-{stem}.int8.onnx",
            "decoder": model_dir / f"decoder-{stem}.onnx",
            "joiner": model_dir / f"joiner-{stem}.int8.onnx",
        }
        self.recognizer = sherpa_onnx.OnlineRecognizer.from_transducer(
            **{key: str(path) for key, path in self.files.items()},
            num_threads=threads, provider="cpu", sample_rate=16000, feature_dim=80,
            decoding_method="greedy_search",
        )


def build_opus(model_dir):
    if {p.name for p in model_dir.iterdir() if p.is_file()} != EXPECTED:
        raise ValueError("only allowlisted Marian files allowed")
    weights = model_dir / "model.safetensors"
    if digest(weights) != OPUS_WEIGHT_HASH:
        raise ValueError("Marian weights mismatch")
    os.environ.update(HF_HUB_OFFLINE="1", TRANSFORMERS_OFFLINE="1", HF_HUB_DISABLE_TELEMETRY="1")
    import torch
    from transformers import MarianMTModel, MarianTokenizer

    torch.set_num_threads(4)
    torch.set_num_interop_threads(1)
    started = time.perf_counter()
    tokenizer = MarianTokenizer.from_pretrained(str(model_dir), local_files_only=True)
    model = MarianMTModel.from_pretrained(str(model_dir), local_files_only=True,
                                            use_safetensors=True).eval()
    load_ms = round((time.perf_counter() - started) * 1000, 1)

    def translator(text):
        if not text:
            return None, None, None
        started = time.perf_counter_ns()
        with torch.inference_mode():
            batch = tokenizer(text, return_tensors="pt", truncation=True, max_length=256)
            result = model.generate(**batch, num_beams=4, do_sample=False, max_new_tokens=120)
        output = tokenizer.decode(result[0], skip_special_tokens=True).strip()
        return output or None, None, time.perf_counter_ns() - started

    return translator, load_ms


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for name in ("audio", "frozen", "output"):
        ap.add_argument("--" + name, required=True, type=Path)
    for name in ("whisper-model", "sherpa-model-dir", "opus-model-dir"):
        ap.add_argument("--" + name, type=Path)
    ap.add_argument("--revision", required=True)
    ap.add_argument("--whisper-port", type=int, default=18080)
    ap.add_argument("--resource-pid", action="append", type=int, default=[])
    args = ap.parse_args()
    if bool(args.whisper_model) == bool(args.sherpa_model_dir):
        ap.error("select exactly one ASR candidate")
    if digest(args.frozen) != FROZEN:
        raise ValueError("the race requires the exact Windows #27 frozen capture manifest")
    if args.whisper_model and digest(args.whisper_model) != TINY_EN:
        raise ValueError("Whisper tiny.en weights mismatch")
    args.asr_only = args.opus_model_dir is None
    if args.asr_only:
        opus, opus_load_ms = lambda _text: (None, None, None), None
    else:
        opus, opus_load_ms = build_opus(args.opus_model_dir)
    args.whisper_version = "v1.8.3" if args.whisper_model else None
    args.whisper_label = "whisper.cpp-tiny.en-ctx512"
    args.threads = 4
    args.translator_blob = None if args.asr_only else args.opus_model_dir / "model.safetensors"
    args.translator_label = None if args.asr_only else "Helsinki-NLP/opus-mt-en-vi"
    args.translator_backend = None if args.asr_only else "transformers-Marian-CPU"
    args.ollama_version = None if args.asr_only else version("transformers")
    args.resource_pid.append(os.getpid())
    run(args, sherpa_type=LargerSherpa, translator=opus, evidence={
        "presegmented_oracle_only": True, "silence_inputs_sent_to_asr": True,
        "exact_audited_input_artifact": 15189618561,
        "asr_only": args.asr_only,
        "opus_revision": None if args.asr_only else OPUS_REV, "opus_cold_load_ms": opus_load_ms,
        "opus_runtime_versions": {} if args.asr_only else {p: version(p) for p in ("transformers", "torch", "sentencepiece", "safetensors")},
        "opus_tokenizer_and_config_sha256": {} if args.asr_only else {name: digest(args.opus_model_dir / name) for name in sorted(EXPECTED)},
    })


if __name__ == "__main__":
    main()
