# HY-MT Windows candidate (diagnostic only)

The Tencent model card lists Vietnamese and an English prompt for language
pairs not involving Chinese. Use that prompt without a system turn and
Tencent's recommended decoding settings, with a fixed seed; compare against
the original LiquidAI text-translation prompt in the same isolated Windows
runner. See [Tencent model card](https://huggingface.co/tencent/HY-MT1.5-1.8B-GGUF).

Tencent's 1.25-bit [GGUF model](https://huggingface.co/tencent/Hy-MT1.5-1.8B-1.25bit-GGUF)
snapshot `3d09c4ce7dc00aa7182cb348a760c7d8f6983224` is 461,860,704 bytes
(440.46 MiB), digest
`93e025c93cc082e73a3f142b757623a8b9cf541c020a8013ca4ee669556860ab`.
It requires STQ1_0 not available in the pinned stock Windows x64 runtime; the
vendor links [open llama.cpp PR #22836](https://github.com/ggml-org/llama.cpp/pull/22836),
whose kernel is introduced for ARM NEON. This run uses Tencent's official
Q4_K_M instead, 1,133,080,512 bytes (1,080.59 MiB), immutable snapshot
`265b2e615a7dc9b06c435dc878829ad99a512ba2`, digest fixed in the runner script.
These sizes are for model files, not the runtime. Check actual Windows load
and inference in the resulting job before claiming support.

Run `python tools/live_translate_bench/hymt/compare_hymt.py check-input` to
verify the original 48 clean cases and independently authored 24 new technical
meeting cases (numbers, negation, names, details). Pipeline command, isolated
elevated Windows host with Python 3.11.9:

```powershell
.\tools\live_translate_bench\hymt\run-hosted.ps1 -Evidence "$env:RUNNER_TEMP\sig4a-hymt-diagnostic"
```

The script verifies original release/both weight digests, records Windows
hardware and revision, then creates the proven program-scoped outbound blocks
before either server is started. No network transcription, microphone or live
meeting input. It runs one pinned model at a time with same resources and
recorded readiness; reported timings are warm text request times, not live
captions or ASR. Treat length/other non-stop outputs as review-required.
Send only the blind artifact to bilingual reviewers; hold engine key and raw
results until scoring is frozen. Grade individual facts and intentional
negation/comparators; flag additions or changed names and supply counts for
each cohort separately. No quality gate is set automatically.
