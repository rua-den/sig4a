# Live Translate experimental Windows helper v1
The Windows dashboard launches the explicitly installed, canonical colocated `live-translate/Sig4a.LiveTranslate.exe` (never PATH). No launch or model load on app/tray startup. Opening the Live Translate page, or using its shortcut, never starts capture. The helper may be launched on explicit Refresh devices or Start; device enumeration does not capture. One persistent helper maximum, one session maximum. Dashboard supervises child/process tree with kill-on-close equivalent, bounded graceful shutdown then kill/reap. The helper must also shut down on parent loss/stdin EOF. No content logs (stdout is protocol only, stderr is dropped or sanitized; no recorder/cloud). No disk path in protocol, no arbitrary endpoint/URL accepted from UI. Models live in a canonical local subfolder provisioned deliberately before Start. No automatic downloads during capture.
 
Each message is a single UTF-8 JSON object followed by LF, maximum 16,384 bytes excluding LF. Bound reads before parse (including malformed unterminated lines), fields individually bounded, never log raw bad input. All messages have `"v":1` and `"type"` string. Commands/events are strictly allowlisted; unknown types fail closed. `session` is a monotonically increasing nonzero safe integer assigned by dashboard; no user data in IDs. Responses echo session, stale events ignored at both ends.
 
Commands:
- `{"v":1,"type":"list"}` emits a single `devices` response, no capture. Each device object is `{"id":"...","name":"...","source":"playback"|"mic","is_default":true|false}`; capped inventory (<=32, id <=512 chars, name <=128). IDs/names UI only, do not log.
- `{"v":1,"type":"start","session":1,"source":"playback"|"mic","device_id":null|"…","translate":false|true}`. Null only for intentional default playback, mic always explicitly selected. Revalidate device flow/availability, only open on accepted Start. Missing local ASR assets -> actionable error before opening. Experimental translation optional, off by default; without available model English remains usable. Duplicate Start while active never opens a new source.
- `{"v":1,"type":"stop","session":1}`: immediately cancel inference and capture, no result/end-of-stream flush; send stopped ACK only when endpoint released; ignore stale stop (must not stop newer session). Idempotent for matching session. UI Pause/Stop/close/switch share the stop path; Pause/stop/switch clears pending content. Resume/source switch never auto-captures, requires Start/Resume click again. Parent enforces timely stop-ACK, kill/reap if stuck.
- `{"v":1,"type":"quit"}`: stop/dispose and exit; no automatic reopen. EOF same.
 
Events:
- `{"v":1,"type":"devices","devices":[...]}`
- `{"v":1,"type":"status","session":1,"state":"started"|"stopped"}`; only `started` lights listening indicator; stopped means disposed.
- `{"v":1,"type":"caption","session":1,"en":"…","vi":null|"…"}`; cap each caption to 2,048 chars. Plain text rendering only, no markup/instructions. VI optional and labeled unreliable. May emit EN first and do not delay EN for translation; avoid retroactive update unless same final item can be correlated safely (optional `seq`).
- `{"v":1,"type":"error","session":0|1,"code":"..."}` with safe fixed tokens only (e.g. `backend_missing`, `model_missing`, `translation_unavailable`, `device_missing`, `device_lost`, `permission_denied`, `busy`, `timeout`, `protocol_error`, `internal`). Session zero only for out-of-session lookup/setup. Terminal error paths release before returning and never fall back to mic; translation_unavailable is nonterminal if English still works.
 
Build baseline with helper disabled/missing must keep OCR/Grammar/Translate/Usage intact, and clearly explain manual model/setup requirements. Tests may use controlled synthetic helpers only under test-only build/code, never shipped as real capture. Documentation must name what was not tested: physical devices, numerical accuracy/quality and long meeting soak remain separate.

## Audio check (no normal session)

* `{"v":1,"type":"check_start","session":12,"source":"playback","device_id":null}` starts
  a dedicated test after the operator clicks **Kiểm tra âm thanh**. The same
  source/selection rules apply as for Start, but `translate` is invalid. A
  normal session and a test session cannot share the device or supersede one
  another. `check_stop` has only v/type/session. A wrong mode or stale generation
  cannot stop the active mode.
* The helper reports `check_status` with v/type/session/state `started` or
  `stopped`. The stopped event also has boolean `completed` (true only for a
  normal full window, false for explicit stop or failure). Status stopped is
  sent only after the tested endpoint has been released.
* While listening, `check_meter` has v/type/session, integer `level` in 0..100
  and boolean cumulative `signal` (within this check only). Live meters are
  at most five per second, apart from a final zero-level aggregate on natural
  stop. `check_model` adds a `state` of `ready` or `unavailable`; such a result
  need not arrive before auto-stop. `check_sample` has a single `en` string
  at most 512 UTF-16 code units. There is no VI event in this mode.
* Native device acquisition has a separate startup limit. A native owner
  thread caps capture at five seconds measured conservatively from native start invocation, independently
  of reading stdout, renderer liveness and ASR; the dashboard Job is a
  second hard-stop boundary. The optional worker may be absent/slow and
  never delays capture or endpoint release. The helper retains at most one
  short in-memory EN sample buffer, uses no network/disk audio sinks, and
  suppresses decoder results after stop, source change or mode termination.

The check does not start a second five-second period if the native start
invocation takes time. Once the device is released, an unfinished optional
decoder gets only a short cleanup grace; a wedged worker forces process exit
without an ACK so the dashboard can release/reap rather than reuse that helper.
