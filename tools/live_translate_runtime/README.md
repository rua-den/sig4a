# Experimental SIG4A Live Translate Windows helper

This is the ordinary local x64 helper for SIG4A, not a capture test executable.
Its private stdin/stdout protocol is version 1. Running it alone without input
does not start audio. Opening the UI or enumerating devices also does not open
an audio stream. It waits for Start; stop/switch/close releases before status.
Playback hears all apps routed to the selected render endpoint. Mic is always
explicit, never a default fallback. Neither capture nor text writes to disk.

From the source repository on Windows (.NET 8 SDK and PowerShell 7):

```powershell
& ./tools/live_translate_runtime/Publish.ps1 -Output 'C:\staged-package\live-translate'
```

The shipped folder is self-contained. Keep the EXE and DLLs together next to
the SIG4A EXEs under `live-translate`; the extracted users do not need any
.NET installation. Models are not bundled. In the dashboard, open **Live Translate → Cài model
tiếng Anh** and accept the native Windows confirmation to download the exact
310,414,022-byte archive (310.4 MB); about 450 MB of free space on a local
NTFS drive is required.
The app does not download or verify on launch. **Kiểm tra model** explicitly
checks a model already installed without network access. Choosing **Hủy**
stops installation before its final atomic publish; if final publication
has already completed the verified model remains installed. Installation
does not start listening or change the optional Vietnamese model.

For administrators or a source checkout, this explicit command uses the same
installer, locks, and checksum verification as the dashboard:

```powershell
pwsh -NoProfile -File .\live-translate\Provision-Model.ps1
```

The helper verifies the pinned upstream Sherpa-ONNX archive and all selected
files, then commits a small guarded marker. An already valid legacy install
is not modified or downloaded again. There is no download during capture;
missing or changed assets prevent normal English capture from starting. The
June 26 English model attribution is copied unchanged. The preexisting
optional audio test can still show a meter without a model.

If the whole computer or process stops unexpectedly before publication, a
private, unreferenced `.sig4a-en-*` directory may remain under
`live-translate/models`; the probe does not adopt or remove unknown data on
a later run. Retry without modifying the old folder or any model marker.
Model changes require restarting capture; use the dashboard status before
relying on captions.

Dependencies: NAudio.Wasapi 2.2.1 MIT, SharpZipLib 1.4.2 MIT (bounded BZip2),
sherpa-onnx 1.12.30 Apache-2.0,
ONNX Runtime 1.23.2 MIT (bundled by the Sherpa Win-x64 native runtime),
and the actual installed build SDK's matching .NET 8 Windows runtime. The
packager verifies notices for the exact runtime archive and vendor docs.
The Sherpa release package metadata links source revision
`6d8c5090ce93f04805165b14c1126ea1a94c0aea`.
ONNX Runtime notices come from the official NuGet archive
`Microsoft.ML.OnnxRuntime/1.23.2`; see `third-party/CHECKSUMS.sha256`.

### Optional offline Vietnamese

Tencent does not sponsor, endorse, or affiliate with SIG4A. The Tencent HY
Community License excludes use in the EU, the UK and South Korea.
Read the Tencent license in `notices/HY-MT1.5-1.8B-LICENSE.txt` before choosing
to run the separate setup. This installs only publisher-pinned local assets
from Tencent and llama.cpp and does not start capture (about 1.13 GB of weights).
Pass the flag only if you accept the publisher terms and are outside the
excluded regions. No IP or location check is made; you assess this yourself:

```powershell
pwsh -NoProfile -File .\live-translate\Provision-Translation.ps1 -AcceptPublisherTerms
```

Translation remains off until enabled and a new Start command. When enabled,
the helper stages one verified local CPU process for the session. It discards
native program stdout/stderr, accepts no translation URL from the UI, verifies
that the actual connected loopback peer is the exact child PID before any
HTTP is written, and sends no recording. A fresh secret authenticates the
private child. The helper probes that unauthenticated inference is denied
and the slot/metrics routes are closed before forwarding any caption. English appears immediately; a VI
caption may update the exact same sequence if it is still current. Missing,
damaged, unavailable or timed-out VI yields `translation_unavailable` and
leaves English working. Stop cancels pending work and reaps the translator
instead of keeping a service active. Neither model is included in the ZIP.

This is not proof of actual microphone/headphone, EN or VI quality, or a
60-minute meeting soak; those require isolated real Windows validation.


## Isolated Windows smoke checks from source

Before provisioning: `& ./tools/live_translate_runtime/Smoke.ps1 -Published $helper`
checks actual product EXE inventory, invalid protocol, missing assets, and EOF.
Then run the packaged Provision-Model script and optionally Provision-Translation.
The test-only assembly is built outside the distributable:

```powershell
& ./tools/live_translate_runtime/Inference-Smoke.ps1 -Published $helper -Scratch "$env:RUNNER_TEMP/product-inference"
# Add -Vietnamese only after explicit optional asset provisioning.
```

This feeds a SAPI synthetic English fixture through the exact product ASR
configuration and validates loopback peer acceptance/rejection. With
`-Vietnamese` (only on a runner confirmed legally eligible for Tencent terms), it also checks the actual authenticated publisher-pinned local
MT process and cancellation of an in-flight translation. No physical capture,
accuracy score, live UI, or long soak evidence is implied.

## Audio-check-specific smoke

`dotnet run -c Release --project tools/live_translate_runtime/tests/ProtocolSmoke.csproj`
covers exact test command allowlists, finite level math and independent five-second
window/early-Stop timing with a deliberately unfinished optional sampler. A
test-only subprocess also runs the production stuck-sampler retirement decision
after a simulated owner release; it must exit without acknowledging the check.
It opens no endpoint.

On an isolated Windows host with a default render output and no actual meeting,
the ordinary packaged helper can consume a synthetic sound through WASAPI:

```powershell
& ./tools/live_translate_runtime/AudioCheck-Smoke.ps1 -Published $helper -Scratch "$env:RUNNER_TEMP/check-meter"
# After explicit English provisioning, no VI setup required:
& ./tools/live_translate_runtime/AudioCheck-Smoke.ps1 -Published $helper -Scratch "$env:RUNNER_TEMP/check-en" -ExpectSample
```

Use `-RequirePlayback` only on a runner where the route is expected. Otherwise
unavailable hardware is explicitly logged as a skip, never a capture pass. The
script measures started-ACK to stopped-ACK with runner slack, not the exact
PCM cutoff. It is not in the distributable and neither opens a microphone nor logs
device names or captions. A hosted virtual endpoint is not physical proof.
