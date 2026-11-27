# Live Translate Phase 1 benchmark (synthetic only)

The predeclared corpus contains 48 English meeting utterances and two digital-silence controls. It is not a recording or fixed voice corpus: Windows SAPI names, locales, rates, audio hashes and speech-end estimates are frozen in a machine-specific sidecar before measurement. SAPI voice variety depends on installed voices and must not be presented as validation on real meeting accents. The speech-end estimate is the last sample with amplitude ≥64; it is not human alignment.

## Actual model smoke (interactive Windows desktop)

Use 64-bit Windows PowerShell 5.1 (SAPI), Python 3.11+ with Tk, pinned whisper.cpp Windows binaries, a pinned `ggml-base.en.bin`, and a pinned local Ollama installation and matching model blob. Prepare binary/model downloads separately; the benchmark does not install or pull anything. In an isolated environment start Whisper with `whisper-server.exe --host 127.0.0.1 --port 8080 --language en --no-gpu -m <path-to-ggml-base.en.bin>`. Do not enable server format conversion. Start Ollama bound to IPv4 localhost with the local model `hf.co/LiquidAI/LFM2.5-VL-450M-GGUF:Q8_0` already installed, ideally with no network egress during inference. Do not point at a shared/profile-sensitive Ollama instance. The adapter only connects directly to IPv4 loopback, bypasses environment proxies and does not follow redirects. Model-file paths passed to the script must be the exact weights given to the respective external service; the tool hashes them but cannot independently attest what an external process loaded.

From the repo root:

```powershell
$b = '.\tools\live_translate_bench'
$out = Join-Path $env:TEMP ('sig4a-live-' + [guid]::NewGuid())
$audio = Join-Path $out 'wav'
$frozen = Join-Path $out 'frozen.json'
$captions = Join-Path $out 'whisper.jsonl'
$whisperModel = 'C:\bench\ggml-base.en.bin' # replace with actual path
$translatorBlob = 'C:\bench\ollama-model-blob' # replace with actual path
$whisperVersion = 'v1.8.3' # record version actually installed
$ollamaVersion = '<installed-version>'
powershell.exe -NoProfile -File "$b\synthesize.ps1" -Directory $audio
python "$b\measure.py" freeze --audio $audio --frozen $frozen
python "$b\measure.py" run --audio $audio --frozen $frozen --output $captions --whisper-model $whisperModel --translator-blob $translatorBlob --whisper-version $whisperVersion --ollama-version $ollamaVersion --revision (git rev-parse HEAD)
python "$b\run_benchmark.py" --results $captions
```

Keep the Tk benchmark window on-screen. Model inference is real; timestamps separate scheduled speech end, recognizer final, translation final, and Tk caption publication. Tk publication is not physical/compositor timing, and presegmented wall-clock-paced WAV with a 700 ms hold is not live VAD. Caption text is intentionally written only for the bundled synthetic input, in the requested result file. Do not use real conversations or mic audio with this harness. No Windows audio endpoint is opened.

The source language warmup and local translation warmup are reported separately. A single inference worker uses a four-segment queue with recorded drops. The current adapter does not attest cold server startup, CPU/RAM, downloads versus package footprint, physical endpoint capture or the planned Sherpa comparator. The visible harness does not replace Phase 2 native overlay verification.

## Accuracy adjudication

After separately reviewing real translated results, supply `--annotations review.jsonl`. Each review line uses a manifest ID, a subset of the predeclared `facts` labels preserved by the translation, and an explicit boolean for critical negation where applicable. Missing reviews cannot result in a pass; facts from missing captions stay in the denominator. A `null` latency percentile means infinity at that rank due to missing captions.

```json
{"id":"U003","facts_matched":["do not","restart","production"],"critical_negation_ok":true}
```

Numeric targets from this harness are provisional. Do not change any Phase 1 status based on validator, HTTP mocks, or hosted/synthetic captures alone.
