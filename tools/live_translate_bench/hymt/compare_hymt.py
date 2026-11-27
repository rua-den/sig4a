"""Diagnostic-only offline English→Vietnamese benchmark for fixed synthetic text."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import random
import time
import urllib.request

HERE = Path(__file__).resolve().parent
HOLDOUT = HERE / "holdout.json"
ORIGINAL = HERE.parent / "fixture-manifest.json"
PINS = {
    ORIGINAL: "31f7b6f106a435ba657f61ebf45d8ba18b8c4474bc5862dd2acfb8bb917e89ca",
    HOLDOUT: "0587aed43c24653513f9811ac7e45921f12dcc5197e5777bd33862ba0eb154f0",
}


def inputs():
    for path, expected in PINS.items():
        if hashlib.sha256(path.read_bytes().replace(b"\r\n", b"\n")).hexdigest() != expected:
            raise ValueError(f"Frozen corpus mismatch: {path.name}")
    data = json.loads(ORIGINAL.read_bytes())
    original = [(r["id"], r["text"], r["facts"], r["critical_negation"])
                for r in data["entries"] if r["kind"] == "utterance"]
    holdout = json.loads(HOLDOUT.read_bytes())["rows"]
    if len(original) != 48 or len(holdout) != 24:
        raise ValueError("Unexpected frozen corpus counts")
    if len({r[0] for r in original + holdout}) != 72:
        raise ValueError("Unexpected duplicate input IDs")
    return original + holdout


class RejectRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise RuntimeError(f"Local inference unexpectedly attempted redirect (HTTP {code})")


OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}), RejectRedirect())
PORT = 18083
GENERATION = {
    "hymt-q4-k-m": {"temperature": 0.7, "top_k": 20, "top_p": 0.6, "repeat_penalty": 1.05, "seed": 7301},
    "lfm-q8": {"temperature": 0},
}


def translate(engine, source):
    if engine == "hymt-q4-k-m":
        messages = [{"role": "user", "content":
                     "Translate the following segment into Vietnamese, without additional explanation.\n\n" + source}]
    else:
        messages = [{"role": "system", "content":
                     "Translate the supplied English speech to Vietnamese. Output only the Vietnamese translation. Preserve names, numbers, and negation."},
                    {"role": "user", "content": source}]
    request = urllib.request.Request(f"http://127.0.0.1:{PORT}/v1/chat/completions",
        data=json.dumps({"model": engine, "messages": messages, "stream": False,
                         "max_tokens": 120, "cache_prompt": False, **GENERATION[engine]}).encode("utf-8"),
        headers={"Content-Type": "application/json"})
    start = time.perf_counter_ns()
    with OPENER.open(request, timeout=60) as response:
        result = json.load(response)
    elapsed = round((time.perf_counter_ns() - start) / 1e6, 2)
    choice = result["choices"][0]
    return {"output": (choice["message"]["content"] or "").strip() or None,
            "request_ms": elapsed, "finish_reason": choice["finish_reason"],
            "usage": result.get("usage")}


def write_rows(path, rows):
    with path.open("x", encoding="utf-8") as fp:
        for row in rows:
            fp.write(json.dumps(row, ensure_ascii=False) + "\n")
            fp.flush()


def run(engine, output):
    cases = inputs()
    # Independent warm-ups and no same-prefix cache sharing between rows.
    for warmup in ("The rehearsal opens at eleven.", "We can discuss the package after lunch."):
        result = translate(engine, warmup)
        if not result["output"] or result["finish_reason"] != "stop":
            raise RuntimeError("A separate synthetic warm-up failed")
    write_rows(output, ({"id": tag, "group": "original48" if tag.startswith("U") else "holdout24",
                         "input": source, "engine": engine, **translate(engine, source)}
                        for tag, source, _, _ in cases))


def grade_packet(directory):
    cases = inputs()
    results = {}
    for engine in GENERATION:
        rows = [json.loads(line) for line in (directory / f"{engine}.jsonl").read_text(
            encoding="utf-8").splitlines()]
        if [(row["id"], row["input"]) for row in rows] != [(tag, source) for tag, source, _, _ in cases]:
            raise ValueError("Rows do not exactly match frozen order and input")
        if any(row["engine"] != engine for row in rows):
            raise ValueError("Incorrect engine provenance")
        results[engine] = {row["id"]: row for row in rows}
    rng = random.Random(7301)
    blind, key = [], []
    for tag, source, facts, negation in cases:
        order = list(GENERATION)
        rng.shuffle(order)
        blind.append({"id": tag, "input": source, "facts": facts, "critical_negation": negation,
                      "A": results[order[0]][tag]["output"], "B": results[order[1]][tag]["output"]})
        key.append({"id": tag, "A": order[0], "B": order[1]})
    write_rows(directory / "blind-grading.jsonl", blind)
    write_rows(directory / "blind-key-do-not-open-before-grading.jsonl", key)
    report = {"kind": "text-only MT timings, not live-caption latency", "python": platform.python_version(),
              "machine": platform.platform(), "fixture_hashes_lf": {path.name: hash for path, hash in PINS.items()},
              "sampling": GENERATION, "models": {}}
    for engine, rows in results.items():
        report["models"][engine] = {}
        for cohort in ("original48", "holdout24"):
            subset = [row for row in rows.values() if row["group"] == cohort]
            times = sorted(row["request_ms"] for row in subset)
            report["models"][engine][cohort] = {
                "rows": len(subset), "nonempty": sum(bool(row["output"]) for row in subset),
                "length_or_nonstop": sum(row["finish_reason"] != "stop" for row in subset),
                "p50_ms": times[math.ceil(len(times)*.5)-1],
                "p95_ms": times[math.ceil(len(times)*.95)-1],
            }
    with (directory / "latency-summary.json").open("x", encoding="utf-8") as fp:
        json.dump(report, fp, indent=2)
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check-input")
    measure = commands.add_parser("run")
    measure.add_argument("engine", choices=GENERATION)
    measure.add_argument("output", type=Path)
    grade = commands.add_parser("grade-packet")
    grade.add_argument("directory", type=Path)
    args = parser.parse_args()
    if args.command == "check-input":
        print(f"Frozen corpus: {len(inputs())} synthetic rows")
    elif args.command == "run":
        run(args.engine, args.output)
    else:
        grade_packet(args.directory)
