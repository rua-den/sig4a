"""Phase 1 synthetic only: score a fixed PCM tape after continuous endpointing and MT."""
from __future__ import annotations

import argparse
import array
from bisect import bisect_right
import hashlib
import json
import math
from pathlib import Path
import queue
import random
import sys
import threading
import time
import tkinter as tk
import wave

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
from measure import read_pcm, json_request
from run_benchmark import MANIFEST, RATE, digest, load

FROZEN = "68fd06d26e24e8f0e25f93e068261e01c67436c7c5dc6da256e2521a7a42f288"
HY_SHA = "4383ac0c3c8e476de98ff979c2a3f069f8c4fb385e7860cf2d28da896cc477c7"
FRAME = RATE // 20
ENDPOINT = {
    "enable_endpoint_detection": True,
    "rule1_min_trailing_silence": 1.8,
    "rule2_min_trailing_silence": 0.60,
    "rule3_min_utterance_length": 18.0,
}
GENERATION = {"temperature": 0.7, "top_k": 20, "top_p": 0.6,
              "repeat_penalty": 1.05, "seed": 7301, "max_tokens": 120, "cache_prompt": False}


class ContinuousSherpa:
    def __init__(self, model_dir):
        from importlib.metadata import version
        import numpy as np
        import sherpa_onnx
        self.np = np
        self.version = version("sherpa-onnx")
        stem = "epoch-99-avg-1-chunk-16-left-128"
        self.files = {"tokens": model_dir / "tokens.txt",
                      "encoder": model_dir / f"encoder-{stem}.int8.onnx",
                      "decoder": model_dir / f"decoder-{stem}.onnx",
                      "joiner": model_dir / f"joiner-{stem}.int8.onnx"}
        self.recognizer = sherpa_onnx.OnlineRecognizer.from_transducer(
            **{key: str(path) for key, path in self.files.items()},
            num_threads=4, provider="cpu", sample_rate=RATE, feature_dim=80,
            decoding_method="greedy_search", **ENDPOINT)


    def stream(self):
        return self.recognizer.create_stream()

    def advance(self, stream, pcm):
        samples = self.np.frombuffer(pcm, dtype="<i2").astype(self.np.float32) / 32768
        stream.accept_waveform(RATE, samples)
        while self.recognizer.is_ready(stream):
            self.recognizer.decode_stream(stream)


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")


def build_tape(args):
    if digest(args.frozen) != FROZEN:
        raise ValueError("only the exact #27 frozen audio fixture is supported")
    frozen = json.loads(args.frozen.read_bytes())
    as_windows = MANIFEST.read_bytes().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
    if frozen["fixture_sha256"] != hashlib.sha256(as_windows).hexdigest():
        raise ValueError("original manifest bytes differ from #27 checkout")
    original = load()["entries"]
    data = bytearray()
    labels = []
    rng = random.Random(926617)

    def position():
        return len(data) // 2

    def gap(ms):
        data.extend(b"\0" * (2 * RATE * ms // 1000))

    def audio(identifier):
        path = args.audio / f"{identifier}.wav"
        meta = frozen["frames"][identifier]
        if digest(path) != meta["sha256"]:
            raise ValueError(f"{identifier}: WAV hash differs from original artifact")
        pcm = read_pcm(path)
        frames = array.array("h", pcm)
        if sys.byteorder != "little":
            frames.byteswap()
        last = meta["speech_end"]
        if not 0 < last <= len(frames):
            raise ValueError("invalid fixture speech end")
        first = next(i for i, value in enumerate(frames[:last]) if abs(value) >= 64)
        return pcm[:last * 2], first, last

    def control(identifier, kind, ms):
        start = position()
        if kind == "silence":
            gap(ms)
        else:
            # Low-amplitude deterministic broadband noise, no spoken material.
            samples = array.array("h", (rng.randint(-540, 540) for _ in range(RATE * ms // 1000)))
            if sys.byteorder != "little":
                samples.byteswap()
            data.extend(samples.tobytes())
        end = position()
        gap(1000 if kind == "noise" else 800)
        labels.append({"id": identifier, "kind": kind, "associated_from_frame": start,
                       "speech_end_frame": end, "associated_until_frame": position()})

    gap(900)
    control("N001", "noise", 3000)
    utterances = [entry for entry in original if entry["kind"] == "utterance"]
    by_id = {e["id"]: e for e in utterances}
    for i, row in enumerate(utterances, 1):
        start = position()
        pcm, begin, end = audio(row["id"])
        data.extend(pcm)
        gap((1350, 1600, 1850)[i % 3])
        labels.append({"id": row["id"], "kind": "utterance", "text": row["text"], "facts": row["facts"],
                       "critical_negation": row["critical_negation"], "associated_from_frame": start,
                       "speech_start_frame": start + begin, "speech_end_frame": start + end,
                       "associated_until_frame": position()})
        if i in (16, 32):
            control("S001" if i == 16 else "S002", "silence", 3500)

    # Reused exact sample snippets with short internal gaps. Separate stress only,
    # never reported as a new held-out measurement or counted in the 48 denominators.
    ids = ("U004", "U013", "U031", "U042", "U045")
    start = position()
    for idx, identifier in enumerate(ids):
        pcm, begin, end = audio(identifier)
        if idx == 0:
            true_begin = position() + begin
        if idx:
            gap(120)
        data.extend(pcm)
    long_end = position()
    gap(2600)
    labels.append({"id": "L001", "kind": "stress", "text": " ".join(by_id[i]["text"] for i in ids),
                   "reused_original_ids": ids, "associated_from_frame": start, "speech_start_frame": true_begin,
                   "speech_end_frame": long_end, "associated_until_frame": position()})
    gap(1000)
    with wave.open(str(args.output / "tape.wav"), "wb") as stream:
        stream.setparams((1, 2, RATE, 0, "NONE", "not compressed"))
        stream.writeframes(data)
    write_json(args.output / "scoring-only-boundaries.json",
               {"fixture_zip_artifact": 15189618561, "frozen_sha256": FROZEN,
                "tape_sha256": digest(args.output / "tape.wav"), "sample_rate": RATE, "rows": labels,
                "method": "fixed before inference; scored afterward only; sample end from frozen +-64 threshold"})


def translate(port, source):
    if not source.strip():
        return None
    payload = {
        "model": "hymt-q4-k-m", "stream": False,
        "messages": [{"role": "user", "content":
                      "Translate the following segment into Vietnamese, without additional explanation.\n\n" + source}],
        **GENERATION,
    }
    response = json_request(port, "/v1/chat/completions", json.dumps(payload).encode(), "application/json")
    choice = response["choices"][0]
    return {"text": (choice["message"]["content"] or "").strip(), "finish_reason": choice["finish_reason"]}


def play(args):
    import os
    import psutil
    if sys.platform != "win32":
        raise ValueError("Use an interactive Windows desktop for the visible caption probe")
    if digest(args.hymt_weights) != HY_SHA:
        raise ValueError("wrong HY-MT weights")
    out = args.output
    path = out / "tape.wav"
    with wave.open(str(path), "rb") as source:
        if (source.getnchannels(), source.getsampwidth(), source.getframerate(),
                source.getcomptype()) != (1, 2, RATE, "NONE") or not 1 <= source.getnframes() <= 450 * RATE:
            raise ValueError("expected a bounded mono PCM16 synthetic tape")
        pcm = source.readframes(source.getnframes())
    model = ContinuousSherpa(args.model_dir)
    for phrase in ("The rehearsal opens at eleven.", "We can discuss the package after lunch."):
        warm = translate(args.port, phrase)
        if not warm["text"] or warm["finish_reason"] != "stop":
            raise RuntimeError("independent MT warmup failed")

    todo = queue.Queue(maxsize=4)
    events = queue.SimpleQueue()
    errors = queue.SimpleQueue()
    finished, stopped = threading.Event(), threading.Event()
    root = tk.Tk()
    root.title("SIG4A continuous synthetic captions")
    root.geometry("720x180")
    caption = tk.Label(root, text="Synthetic captions only", wraplength=675, font=("Segoe UI", 21))
    caption.pack(fill="both", expand=True)
    root.update()
    processes = [psutil.Process(os.getpid()), psutil.Process(args.server_pid)]
    cpu_start = sum(sum(p.cpu_times()[:2]) for p in processes)
    anchor = time.perf_counter_ns()
    rss = []
    trace = []

    def now_ms():
        return round((time.perf_counter_ns() - anchor) / 1e6, 3)

    def capture():
        recognizer = model.recognizer
        stream = model.stream()
        try:
            for offset in range(0, len(pcm), FRAME * 2):
                frame_end = min(len(pcm), offset + FRAME * 2)
                target = frame_end * 1000 / (RATE * 2)
                delay = max(0.0, (target - now_ms()) / 1000)
                if stopped.wait(delay):
                    return
                fed = now_ms()
                model.advance(stream, pcm[offset:frame_end])
                decoded = now_ms()
                trace.append((frame_end // 2, target, fed, decoded))
                if not recognizer.is_endpoint(stream):
                    continue
                text = recognizer.get_result(stream).strip()
                recognizer.reset(stream)
                if not text:
                    continue
                row = {"endpoint_frame": frame_end // 2, "endpoint_ms": decoded,
                       "transcript": text, "queue_depth_before": todo.qsize(),
                       "visible_final_caption_ms": None}
                try:
                    todo.put_nowait(row)
                except queue.Full:
                    events.put({**row, "error": "translation_queue_full"})
            # No fixture-oracle flush: final labels require a normal endpoint.
            dangling = recognizer.get_result(stream).strip()
            if dangling:
                events.put({"endpoint_frame": len(pcm) // 2, "endpoint_ms": now_ms(), "transcript": dangling,
                            "error": "end_of_tape_without_detected_endpoint", "visible_final_caption_ms": None})
        except Exception as exc:
            errors.put(exc)
        finally:
            finished.set()

    def worker():
        while not stopped.is_set() and (not finished.is_set() or not todo.empty()):
            try:
                row = todo.get(timeout=.1)
            except queue.Empty:
                continue
            try:
                row["translation_start_ms"] = now_ms()
                result = translate(args.port, row["transcript"])
                row["translation_final_ms"] = now_ms()
                if result["finish_reason"] != "stop" or not result["text"]:
                    row["error"] = "invalid_or_incomplete_translation"
                else:
                    row["translation"] = result["text"]
            except Exception as exc:
                row["error"] = type(exc).__name__
            events.put(row)

    feeder = threading.Thread(target=capture, daemon=True)
    consumer = threading.Thread(target=worker, daemon=True)
    with (out / "dynamic-events.jsonl").open("x", encoding="utf-8") as fp:
        def sample():
            rss.append(sum(p.memory_info().rss for p in processes))
            root.after(250, sample)

        def poll():
            while True:
                try:
                    row = events.get_nowait()
                except queue.Empty:
                    break
                if row.get("translation"):
                    caption.configure(text=row["translation"])
                    root.update_idletasks()
                    row["visible_final_caption_ms"] = now_ms()
                fp.write(json.dumps(row, ensure_ascii=False) + "\n")
                fp.flush()
            if consumer.is_alive() or not events.empty():
                root.after(8, poll)
            else:
                root.after(300, root.destroy)

        root.after(0, sample)
        root.after(8, poll)
        feeder.start()
        consumer.start()
        root.mainloop()
    stopped.set()
    if feeder.is_alive() or consumer.is_alive() or not events.empty():
        raise RuntimeError("Proof window closed before caption worker drained")
    if not errors.empty():
        raise RuntimeError("continuous endpoint path failed") from errors.get()

    write_json(out / "frame-trace.json", {"columns": ["input_end_frame", "scheduled_ms", "fed_ms", "decoded_ms"],
                                         "rows": trace})
    elapsed = now_ms() / 1000
    write_json(out / "run-provenance.json", {
        "tape_sha256": digest(path), "event_sha256": digest(out / "dynamic-events.jsonl"),
        "trace_sha256": digest(out / "frame-trace.json"), "endpoint_defaults": ENDPOINT,
        "scope": "continuously paced prerecorded synthetic PCM; native local endpoint rule; no oracle sectioning, no device capture",
        "elapsed_ms": elapsed * 1000, "caption_timestamp_method": "Tk label update_idletasks (not compositor) from common monotonic start",
        "sherpa_version": model.version, "sherpa_files": {key: digest(value) for key, value in model.files.items()},
        "translation_weight_sha256": HY_SHA, "translation_generation": GENERATION, "git_revision": args.revision,
        "feed_frame_ms": 50, "queue_capacity": 4, "max_combined_rss_bytes": max(rss),
        "combined_cpu_cores": (sum(sum(p.cpu_times()[:2]) for p in processes) - cpu_start) / elapsed,
    })


def score(args):
    out = args.output
    boundaries = json.loads((out / "scoring-only-boundaries.json").read_bytes())
    run = json.loads((out / "run-provenance.json").read_bytes())
    if not (boundaries["tape_sha256"] == run["tape_sha256"] == digest(out / "tape.wav")
            and run["event_sha256"] == digest(out / "dynamic-events.jsonl")):
        raise ValueError("score evidence hash mismatch")
    events = [json.loads(line) for line in (out / "dynamic-events.jsonl").read_text(encoding="utf8").splitlines()]
    labels = boundaries["rows"]
    starts = [r["associated_from_frame"] for r in labels]
    buckets = [[] for _ in labels]
    unmatched = []
    for event in events:
        frame = event["endpoint_frame"]
        chosen = bisect_right(starts, frame - 1) - 1
        if (chosen >= 0 and frame <= labels[chosen]["associated_until_frame"]):
            buckets[chosen].append(event)
        else:
            unmatched.append(event)

    report_rows, lags = [], []
    for label, group in zip(labels, buckets):
        row = {**label, "events": group}
        good = [e for e in group if e.get("visible_final_caption_ms") is not None and not e.get("error")]
        if label["kind"] == "utterance":
            # Keep split snapshots in evidence, but a synthetic utterance with no
            # single curtain-ready translation is unscored, never omitted.
            value = math.inf
            if len(group) == len(good) == 1:
                value = good[0]["visible_final_caption_ms"] - label["speech_end_frame"] * 1000 / RATE
                if value < 0:
                    row["early_final"] = True
                    value = math.inf
                else:
                    row["caption_for_review"] = good[0]["translation"]
            lags.append(value)
            row["visible_latency_ms"] = value if math.isfinite(value) else None
        report_rows.append(row)
    lags.sort()

    def rank(p):
        value = lags[math.ceil(len(lags) * p) - 1]
        return value if math.isfinite(value) else None

    report = {"ungated_synthetic_only": True, "original_utterances": len(lags),
              "single_visible_translations": sum(math.isfinite(n) for n in lags),
              "missing_duplicate_or_early_counted_as_infinite": sum(not math.isfinite(n) for n in lags),
              "p50_ms": rank(.5), "p95_ms": rank(.95),
              "noise_or_silence_endpoints": sum(len(row["events"]) for row in report_rows if row["kind"] in ("noise", "silence")),
              "manual_Vietnamese_fact_score": None, "unmatched_endpoints": unmatched,
              "stress": [row for row in report_rows if row["kind"] == "stress"]}
    write_json(out / "timing-diagnostic.json", report)
    write_json(out / "caption-review-packet.json", report_rows)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    cmd = parser.add_subparsers(dest="command", required=True)
    for action in ("prepare", "play", "score"):
        sub = cmd.add_parser(action)
        sub.add_argument("--output", type=Path, required=True)
        if action == "prepare":
            sub.add_argument("--audio", type=Path, required=True)
            sub.add_argument("--frozen", type=Path, required=True)
        if action == "play":
            sub.add_argument("--model-dir", type=Path, required=True)
            sub.add_argument("--hymt-weights", type=Path, required=True)
            sub.add_argument("--server-pid", type=int, required=True)
            sub.add_argument("--port", type=int, default=18083)
            sub.add_argument("--revision", required=True)
    opts = parser.parse_args()
    {"prepare": build_tape, "play": play, "score": score}[opts.command](opts)
