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
.NET installation. Models are not bundled and must be installed on purpose:

```powershell
pwsh -NoProfile -File .\live-translate\Provision-Model.ps1
```

The installer uses Windows curl/tar, downloads the pinned Sherpa-ONNX artifact
from upstream (~200 MB), verifies the archive and individual files, and
atomically stages the English offline model. There is no download during
capture. Missing or changed assets prevent hardware from starting.
The June 26 English model attribution from the archive is copied unchanged.

Dependencies: NAudio.Wasapi 2.2.1 MIT, sherpa-onnx 1.12.30 Apache-2.0,
ONNX Runtime 1.23.2 MIT (bundled by the Sherpa Win-x64 native runtime),
and the actual installed build SDK's matching .NET 8 Windows runtime. The
packager verifies notices for the exact runtime archive and vendor docs.
The Sherpa release package metadata links source revision
`6d8c5090ce93f04805165b14c1126ea1a94c0aea`.
ONNX Runtime notices come from the official NuGet archive
`Microsoft.ML.OnnxRuntime/1.23.2`; see `third-party/CHECKSUMS.sha256`.

### Optional offline Vietnamese

Read the Tencent license in `notices/HY-MT1.5-1.8B-LICENSE.txt` before choosing
to run the separate setup. This installs only publisher-pinned local assets
from Tencent and llama.cpp and does not start capture (about 1.13 GB of weights):

```powershell
pwsh -NoProfile -File .\live-translate\Provision-Translation.ps1
```

Translation remains off until enabled and a new Start command. When enabled,
the helper stages one verified local CPU process for the session. It discards
native program stdout/stderr, accepts no translation URL from the UI, verifies
that the actual connected loopback peer is the exact child PID before any
HTTP is written, and sends no recording. English appears immediately; a VI
caption may update the exact same sequence if it is still current. Missing,
damaged, unavailable or timed-out VI yields `translation_unavailable` and
leaves English working. Stop cancels pending work and reaps the translator
instead of keeping a service active. Neither model is included in the ZIP.

This is not proof of actual microphone/headphone, EN or VI quality, or a
60-minute meeting soak; those require isolated real Windows validation.
