# Live Translate native WASAPI probe

This independent Phase 1 diagnostic opens one explicitly selected Windows Core
Audio endpoint at a time. Playback is render endpoint loopback (all apps going
to that output, including notifications). Mic is explicitly selected capture
endpoint. Mic requires an exact endpoint ID; playback may use the default
multimedia endpoint or an exact ID. There is no cross-source fallback.
It does not use FFmpeg or write/transmit samples. Reports only endpoint
identity, timing, sample counters and scalar meter observations.

Windows with .NET 8 SDK and PowerShell 7, from the repository root:

```powershell
$dir = './tools/live_translate_audio_probe'
dotnet build "$dir/AudioProbe.csproj" -c Release
& "$dir/probe.ps1" -Source list
& "$dir/probe.ps1" -Source playback -Seconds 5
& "$dir/probe.ps1" -Source mic -Device '<exact capture ID>' -Seconds 5
& "$dir/probe.ps1" -Source playback -Device '<exact render ID>' -Then mic -ThenDevice '<exact capture ID>' -Seconds 5
```

Use a child PowerShell process if you need to collect/check exit codes without
exiting the calling script. For every selected source the probe stops, receives
the WASAPI recording-stopped notification, disposes the stream, then opens and
closes a new stream (marked `reopen`) using the exact same ID. Opening in shared
mode alone is not proof of exclusive ownership; the stop notification, disposal
and non-overlapping timestamps are intended lifecycle evidence. The wrapper
kills a stuck probe after a bounded timeout, which is a failure, not release
success. An empty playback stream is possible when nothing renders. Signal
alone is not proof of voice recognition, physical hardware, or isolation.

## Hosted Windows CI candidate

Compile, enumerate devices, and run playback against a known isolated endpoint
with a synthetic tone routed to it. If the runner has zero active endpoints,
report this as unavailable, not a pass. If it has alternate/virtual endpoints,
test only with a synthetic fixture known to be routed to them. Log JSON and
exit code; require two successful measured signal rows on the same endpoint
and disposal before reopen for a plumbing pass. Wrong-flow ID and nonexistent ID
must fail without returning a success record. Do not mark the hardware gate
passed for any hosted/virtual result.

## Physical validation (operator in control)

Use an empty test session, with no actual meeting. Identify a real wired/USB
headphone output and physical microphone from Windows Sound settings and the
list; use exact endpoint IDs. Play a known synthetic tone through the headphones
while the mic is muted for playback-only. Stop the tone, then speak a short
synthetic phrase into the mic while the playback endpoint is silent for
mic-only. Verify endpoint identity, positive level, and both reopen records
in each run. Test cross-source isolation separately: with mic hardware muted,
keep playback tone running and verify mic shows no signal; with headphones
silent, speak into the mic and verify playback shows no signal. Record routing,
hardware, observed signal and disposal timestamps; keep audio/transcript out of
test results. Normal mic noise floors can need a hardware-calibrated threshold;
report actual meters and do not infer isolation solely from a Boolean.

## Optional operator package: license provenance

The Windows packaging workflow bundles upstream NAudio 2.2.1 MIT text,
validated against its checked-in SHA-256 at immutable upstream revision
`b5d5ff83fd378f046398891fe5cd99426ce44732`. It also bundles the unmodified
LICENSE.TXT and THIRD-PARTY-NOTICES.TXT from the same Microsoft Windows x64 .NET
8 runtime pack used by the self-contained publish, not the notice files from
whatever machine reads the ZIP. Packaging resolves the exact runtime version
from the build's NuGet assets, requires the archive to match the cached
NuGet SHA-512 and records the package URL, members and individual notice hashes
in `notices/notice-provenance.json`. No additional runtime or network access is
required to view those notices.
