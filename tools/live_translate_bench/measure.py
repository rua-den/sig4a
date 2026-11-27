"""Windows Phase 1 synthetic-file adapter. No real mic/input or remote network."""
from __future__ import annotations

import argparse
import array
import http.client
import io
import json
import os
from pathlib import Path
import platform
import queue
import sys
import threading
import time
import tkinter as tk
import wave

from run_benchmark import GAP_MS, MANIFEST, digest, load, validate

RATE = 16000
CHUNK = RATE // 10
TRANSLATOR = "hf.co/LiquidAI/LFM2.5-VL-450M-GGUF:Q8_0"


def json_request(port, path, content, content_type):
    # http.client does not use environment proxy or follow redirects.
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=15)
    try:
        connection.request("POST", path, content, {"Content-Type": content_type})
        response = connection.getresponse()
        if response.status != 200:
            raise RuntimeError(f"{path}: HTTP {response.status}")
        raw = response.read(1024 * 1024 + 1)
        if len(raw) > 1024 * 1024:
            raise RuntimeError("Backend response too large")
        return json.loads(raw)
    finally:
        connection.close()


def wav_bytes(pcm):
    target = io.BytesIO()
    with wave.open(target, "wb") as out:
        out.setparams((1, 2, RATE, 0, "NONE", "not compressed"))
        out.writeframes(pcm)
    return target.getvalue()


def whisper(port, pcm):
    boundary = "sig4a-" + os.urandom(16).hex()
    parts = []
    for key, value in {"response_format": "json", "temperature": "0.0",
                       "temperature_inc": "0.0"}.items():
        parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{key}"\r\n\r\n{value}\r\n'.encode())
    parts += [f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="fixture.wav"\r\nContent-Type: audio/wav\r\n\r\n'.encode(),
              wav_bytes(pcm), f'\r\n--{boundary}--\r\n'.encode()]
    payload = json_request(port, "/inference", b"".join(parts), f"multipart/form-data; boundary={boundary}")
    return payload["text"].strip()


def translate(port, source, backend="ollama"):
    if not source:
        return None, None, None
    payload = {
        "model": TRANSLATOR,
        "messages": [{"role": "system", "content":
                      "Translate the supplied English speech to Vietnamese. Output only the Vietnamese translation. Preserve names, numbers, and negation."},
                     {"role": "user", "content": source}],
        "stream": False, "keep_alive": "10m",
        "options": {"temperature": 0, "num_predict": 120},
    }
    if backend == "llama.cpp":
        payload.update(model="sig4a-lfm-q8", temperature=0, max_tokens=120)
        payload.pop("keep_alive")
        payload.pop("options")
        r = json_request(port, "/v1/chat/completions", json.dumps(payload).encode(), "application/json")
        return r["choices"][0]["message"]["content"].strip() or None, None, None
    r = json_request(port, "/api/chat", json.dumps(payload).encode(), "application/json")
    value = r["message"]["content"].strip()
    return value or None, r.get("load_duration"), r.get("eval_duration")


def read_pcm(path):
    with wave.open(str(path), "rb") as w:
        if (w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getcomptype()) != (1, 2, RATE, "NONE"):
            raise ValueError(f"{path.name}: require mono PCM16 at 16 kHz")
        if not 0 < w.getnframes() <= RATE * 15:
            raise ValueError(f"{path.name}: invalid duration")
        return w.readframes(w.getnframes())


def freeze(args):
    manifest = load()
    if errors := validate(manifest):
        raise ValueError(errors)
    voices = json.loads((args.audio / "sapi-voices.json").read_text(encoding="utf-8-sig"))
    meta = {"fixture_sha256": digest(MANIFEST), "sapi_voices": voices,
            "onset_end_method": "last frame with abs(PCM16) >= 64; synthetic energy proxy, inspect before scoring",
            "windows": platform.platform(), "frames": {}}
    for row in [{"id": "WARM", "kind": "utterance"}] + manifest["entries"]:
        if row["kind"] == "silence":
            continue
        name = row["id"]
        path = args.audio / f"{name}.wav"
        pcm = read_pcm(path)
        samples = array.array("h", pcm)
        if sys.byteorder != "little":
            samples.byteswap()
        end = next((i + 1 for i in range(len(samples) - 1, -1, -1) if abs(samples[i]) >= 64), 0)
        if not end:
            raise ValueError(f"{name}: no active synthetic samples")
        meta["frames"][name] = {"sha256": digest(path), "speech_end": end}
    with args.frozen.open("x", encoding="utf-8") as out:
        json.dump(meta, out, indent=2)


def run(args, *, sherpa_type=None, translator=None, evidence=None):
    if platform.system() != "Windows":
        raise RuntimeError("Use a Windows desktop for this runner")
    manifest = load()
    frozen = json.loads(args.frozen.read_bytes())
    if frozen["fixture_sha256"] != digest(MANIFEST):
        raise ValueError("fixture manifest changed after freeze")
    if args.output.exists() or args.output.with_suffix(".metadata.json").exists():
        raise FileExistsError("New benchmark runs must use fresh output names")

    def audio(name):
        path = args.audio / f"{name}.wav"
        expected = frozen["frames"][name]
        if digest(path) != expected["sha256"]:
            raise ValueError(f"{name}: synthetic WAV differs from frozen input")
        pcm = read_pcm(path)
        end = expected["speech_end"]
        if not 0 < end <= len(pcm) // 2:
            raise ValueError(f"{name}: invalid frozen speech end")
        # Hold 700 ms after the last active sample before scheduling recognition.
        return pcm[:end * 2] + b"\0" * (2 * RATE * GAP_MS // 1000), end

    entries = []
    for row in manifest["entries"]:
        if row["kind"] == "silence":
            entries.append((row, b"\0" * (2 * RATE * row["duration_ms"] // 1000), 0))
        else:
            entries.append((row, *audio(row["id"])))
    if translator is None:
        translator = lambda text: translate(args.ollama_port, text, args.translator_backend)
    sherpa = None
    cold_model_start = time.monotonic()
    if args.sherpa_model_dir:
        from sherpa_adapter import Sherpa
        if args.whisper_model or args.whisper_version:
            raise ValueError("select one ASR candidate per run")
        sherpa = (sherpa_type or Sherpa)(args.sherpa_model_dir, args.threads)
        recognizer, model_files = sherpa.recognize, sherpa.files
        engine_name, version = sherpa.engine_id, sherpa.version
        asr_model_init = round((time.monotonic() - cold_model_start) * 1000)
    else:
        if not args.whisper_model or not args.whisper_version:
            raise ValueError("Whisper model and exact version are required for the Whisper candidate")
        recognizer = lambda pcm: whisper(args.whisper_port, pcm)
        model_files = {"model": args.whisper_model}
        engine_name, version = getattr(args, "whisper_label", "whisper.cpp-base.en"), args.whisper_version
        asr_model_init = None  # external prestarted server

    # Warm the recognizer and translator separately on unscored synthetic speech.
    cold = time.monotonic()
    asr_warm = recognizer(audio("WARM")[0])
    asr_ready = time.monotonic()
    if not asr_warm:
        raise RuntimeError("ASR warmup returned no speech")
    if not getattr(args, "asr_only", False) and not translator("This is a synthetic warmup sentence.")[0]:
        raise RuntimeError("Translator warmup returned no text")
    warmed = time.monotonic()
    metadata = {
        "machine": platform.machine(), "os": platform.platform(), "cpu_logical": os.cpu_count(),
        "asr_adapter": ("sherpa-onnx native streaming 100 ms frames with known final boundary"
                        if sherpa else "whisper.cpp HTTP segmented WAV, not native streaming"),
        "asr_engine": engine_name, "asr_version": version,
        "asr_model_init_ms_in_process": asr_model_init,
        "translation_model": getattr(args, "translator_label", TRANSLATOR), "synthetic_only": True,
        "translator_backend": args.translator_backend, "translator_version": args.ollama_version,
        "repository_revision": args.revision,
        "asr_models": {key: {"sha256": digest(path), "bytes": path.stat().st_size}
                       for key, path in model_files.items()},
        "translator_model_sha256": digest(args.translator_blob) if args.translator_blob else None,
        "translator_model_bytes": args.translator_blob.stat().st_size if args.translator_blob else None,
        "fixture_sha256": frozen["fixture_sha256"], "frozen_sha256": digest(args.frozen),
        "asr_warmup_ms_model_prestarted": round((asr_ready - cold) * 1000),
        "translator_warmup_ms": round((warmed - asr_ready) * 1000),
        "metric_limit": "Tk update_idletasks publication proxy, not compositor or SIG4A overlay proof; scheduled WAV sections, no live segmentation gate",
    }
    metadata.update(evidence or {})
    args.output.with_suffix(".metadata.json").write_text(json.dumps(metadata, indent=2), encoding="utf-8")
    todo = queue.Queue(maxsize=4)
    results = queue.Queue(maxsize=64)
    finished, canceled = threading.Event(), threading.Event()
    root = tk.Tk()
    root.title("SIG4A synthetic captions benchmark")
    root.geometry("720x180")
    caption = tk.Label(root, text="Synthetic captions only", wraplength=675, font=("Segoe UI", 21))
    caption.pack(fill="both", expand=True)
    root.update()
    started = time.monotonic()
    resources = None
    if args.resource_pid:
        import psutil
        processes = [psutil.Process(pid) for pid in set([os.getpid(), *args.resource_pid])]

        def snapshot():
            return [tuple(proc.cpu_times())[:2] for proc in processes]

        initial_cpu = snapshot()
        rss_total = []
        resources = {"max_total_rss_bytes": 0}

    def now_ms():
        return round((time.monotonic() - started) * 1000)

    def wait_until(target):
        while now_ms() < target:
            if canceled.wait(min(0.1, max(0, target - now_ms()) / 1000)):
                return False
        return not canceled.is_set()

    producer_errors = queue.SimpleQueue()

    def produce():
        position = 0
        try:
            for row, pcm, end in entries:
                # Replay time advances independently of inference; audio is delivered to the
                # segmented adapter only once the complete window has elapsed.
                ready = round((position + len(pcm) // 2) * 1000 / RATE)
                speech_end = round((position + end) * 1000 / RATE)
                begin_asr = None
                transcript = None
                if sherpa:
                    stream = sherpa.stream()
                    for offset in range(0, len(pcm), CHUNK * 2):
                        frame = pcm[offset:offset + CHUNK * 2]
                        frame_ready = round((position + (offset + len(frame)) // 2) * 1000 / RATE)
                        if not wait_until(frame_ready):
                            return
                        if begin_asr is None:
                            begin_asr = now_ms()
                        sherpa.advance(stream, frame)
                    transcript = sherpa.finish(stream)
                elif not wait_until(ready):
                    return
                position += len(pcm) // 2
                if canceled.is_set():
                    return
                data = {"id": row["id"], "engine": engine_name, "speech_end_ms": speech_end,
                        "input_ready_ms": ready, "visible_final_caption_ms": None, "queue_depth": todo.qsize()}
                if sherpa:
                    data.update(transcript=transcript, recognition_start_ms=begin_asr,
                                recognition_final_ms=now_ms())
                try:
                    todo.put_nowait((data, pcm))
                except queue.Full:
                    results.put({**data, "error": "queue_full"})
        except Exception as exc:
            producer_errors.put(exc)
        finally:
            finished.set()

    def infer():
        while not canceled.is_set() and (not finished.is_set() or not todo.empty()):
            try:
                data, pcm = todo.get(timeout=0.1)
            except queue.Empty:
                continue
            try:
                if not sherpa:
                    data["recognition_start_ms"] = now_ms()
                    data["transcript"] = recognizer(pcm)
                    data["recognition_final_ms"] = now_ms()
                data["translation"], data["translation_load_ns"], data["translation_eval_ns"] = translator(data["transcript"])
                data["translation_final_ms"] = now_ms()
            except (OSError, ValueError, KeyError, RuntimeError, http.client.HTTPException) as error:
                data["error"] = type(error).__name__
            results.put(data)

    p = threading.Thread(target=produce, daemon=True)
    w = threading.Thread(target=infer, daemon=True)
    with args.output.open("x", encoding="utf-8") as log:
        def sample_resources():
            if resources is None:
                return
            current_rss = sum(proc.memory_info().rss for proc in processes)
            rss_total.append(current_rss)
            root.after(250, sample_resources)

        def poll():
            try:
                record = results.get_nowait()
            except queue.Empty:
                record = None
            if record is not None:
                if text := record.get("translation"):
                    caption.configure(text=text)
                    root.update_idletasks()
                    record["visible_final_caption_ms"] = now_ms()
                log.write(json.dumps(record, ensure_ascii=False) + "\n")
                log.flush()
            if w.is_alive() or not results.empty():
                root.after(8, poll)
            else:
                root.after(400, root.destroy)
        root.after(8, poll)
        root.after(0, sample_resources)
        p.start()
        w.start()
        root.mainloop()
        incomplete = w.is_alive() or not results.empty()
        canceled.set()
    if not producer_errors.empty():
        raise RuntimeError("synthetic input producer failed") from producer_errors.get()
    if incomplete:
        raise RuntimeError("benchmark window closed before completion")
    if resources is not None:
        elapsed = time.monotonic() - started
        resources = {
            "pids": [proc.pid for proc in processes],
            "rss_samples": len(rss_total),
            "max_total_rss_bytes": max(rss_total, default=0),
            "mean_cpu_cores": (sum(sum(v) for v in snapshot()) - sum(sum(v) for v in initial_cpu)) / elapsed,
            "windows_counts_shared_pages_in_each_process_rss": True,
        }
    metadata.update(resources=resources, result_sha256=digest(args.output))
    args.output.with_suffix(".metadata.json").write_text(json.dumps(metadata, indent=2), encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest="action", required=True)
    for action in ("freeze", "run"):
        sub = subs.add_parser(action)
        sub.add_argument("--audio", required=True, type=Path)
        sub.add_argument("--frozen", required=True, type=Path)
        if action == "run":
            sub.add_argument("--output", required=True, type=Path)
            sub.add_argument("--whisper-model", type=Path)
            sub.add_argument("--sherpa-model-dir", type=Path)
            sub.add_argument("--resource-pid", action="append", type=int, default=[])
            sub.add_argument("--threads", type=int, choices=range(1, 17), default=4)
            sub.add_argument("--translator-blob", required=True, type=Path)
            sub.add_argument("--whisper-version")
            sub.add_argument("--ollama-version", "--translator-version", dest="ollama_version", required=True)
            sub.add_argument("--translator-backend", choices=("ollama", "llama.cpp"), default="ollama")
            sub.add_argument("--revision", required=True)
            sub.add_argument("--whisper-port", type=int, default=8080)
            sub.add_argument("--ollama-port", "--translator-port", dest="ollama_port", type=int, default=11434)
    args = parser.parse_args()
    (freeze if args.action == "freeze" else run)(args)


if __name__ == "__main__":
    main()
