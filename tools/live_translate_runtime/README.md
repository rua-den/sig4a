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

English is experimental. Current first implementation reports
`translation_unavailable` if asked for Vietnamese and continues English.
This is not proof of actual microphone/headphone, EN or VI quality, or a
60-minute meeting soak; those require isolated real Windows validation.
