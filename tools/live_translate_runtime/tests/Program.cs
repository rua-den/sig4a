using System.Text;
using System.Diagnostics;
using System.Text.Json;

if (args is ["--stuck-sampler"])
{
    // Test executable only: the real guard exit is called with a stuck decoder,
    // after a synthetic owner release marker. No WASAPI or model is loaded.
    Console.WriteLine("capture-owner-release-simulated");
    CheckTiming.RetireSampleOrExit(new TaskCompletionSource().Task);
    Console.WriteLine("unexpected-ack");
    return;
}

static void Check(bool condition) { if (!condition) throw new Exception("Unexpected protocol result."); }
static Protocol.Command Read(string input) => Protocol.Parse(Encoding.UTF8.GetBytes(input));
static void Invalid(Func<Protocol.Command> f)
{
    try { _ = f(); throw new Exception("Expected protocol rejection."); }
    catch (Exception ex) when (ex is FormatException or JsonException) { }
}

Check(Read("""{"v":1,"type":"list"}""").Type == "list");
Check(Read("""{"v":1,"type":"quit"}""").Type == "quit");
Check(Read("""{"v":1,"type":"start","source":"playback","session":12,"device_id":null,"translate":false}""").Device is null);
Check(Read("""{"v":1,"type":"start","source":"mic","session":12,"device_id":"selected","translate":true}""").Translate);
Check(Read("""{"v":1,"type":"stop","session":9007199254740991}""").Session == Protocol.SessionLimit);
foreach (string bad in new [] {
    """{"v":1,"type":"list","path":"stray"}""", """{"v":1,"type":"cloud"}""",
    """{"v":1,"v":1,"type":"list"}""", """{"v":1,"type":"stop","session":9007199254740992}""",
    """{"v":1,"type":"start","source":"mic","session":1,"device_id":null,"translate":true}""",
    """{"v":1,"type":"start","source":"playback","session":1,"device_id":null,"translate":"yes"}""",
    """{"v":1,"type":"start","source":"mic","session":1,"device_id":"\u0000","translate":true}"""
}) Invalid(() => Read(bad));

var multi = new Protocol.BoundedReader(new MemoryStream(Encoding.UTF8.GetBytes("first\nsecond\n")));
Check(Encoding.UTF8.GetString((await multi.Read())!.Value.Span) == "first");
Check(Encoding.UTF8.GetString((await multi.Read())!.Value.Span) == "second");
Check(await multi.Read() is null);
var max = new Protocol.BoundedReader(new MemoryStream(Encoding.UTF8.GetBytes(new string('x', Protocol.LineLimit) + "\n")));
Check((await max.Read())?.Length == Protocol.LineLimit);
foreach (var invalid in new[] { new string('x', Protocol.LineLimit + 1), "unterminated" })
{
    var reader = new Protocol.BoundedReader(new MemoryStream(Encoding.UTF8.GetBytes(invalid)));
    try { await reader.Read(); throw new Exception("Expected bounded reader rejection."); }
    catch (FormatException) { }
}
Console.WriteLine("Compiler-level protocol checks passed; no endpoints or inference exercised.");


Check(Read("""{"v":1,"type":"check_start","source":"mic","session":5,"device_id":"selected"}""") ==
    new Protocol.Command("check_start", 5, "mic", "selected"));
Check(Read("""{"v":1,"type":"check_stop","session":5}""").Type == "check_stop");
Invalid(() => Read("""{"v":1,"type":"check_start","source":"playback","session":5,"device_id":null,"translate":true}"""));
Invalid(() => Read("""{"v":1,"type":"check_start","source":"mic","session":5,"device_id":null}"""));
Invalid(() => Read("""{"v":1,"type":"check_stop","session":5,"source":"mic"}"""));
Invalid(() => Read("""{"v":1,"type":"check_stop","session":0}"""));
Check(CheckTiming.Meter(0, 1, 1) == 0);
Check(CheckTiming.Meter(double.PositiveInfinity, 1, 1) == 0);
Check(CheckTiming.Meter(double.NaN, 1, 1) == 0);
Check(CheckTiming.Meter(1, 1, 1) == 100);
Check(CheckTiming.Meter(.01, 1, 1) is > 60 and < 70);
using var cancelled = new ManualResetEvent(false);
using var driver = new ManualResetEvent(false);
// The callback intentionally never finishes until after the exact native five-second
// capture window has elapsed. Capture timing and release do not depend on it.
var delayedDecoder = new TaskCompletionSource();
var elapsed = System.Diagnostics.Stopwatch.StartNew();
Check(CheckTiming.WaitWindow(cancelled, driver) == WaitHandle.WaitTimeout);
Check(elapsed.Elapsed.TotalSeconds >= 4.8 && elapsed.Elapsed.TotalSeconds < 8);
Check(!delayedDecoder.Task.IsCompleted);
Check(!CheckTiming.CanReuseAfterSample(delayedDecoder.Task)); // must exit, not ACK/reuse
delayedDecoder.SetResult();
Check(CheckTiming.CanReuseAfterSample(delayedDecoder.Task));
long delayedStart = System.Diagnostics.Stopwatch.GetTimestamp() -
    (long)(System.Diagnostics.Stopwatch.Frequency * 2.0);
Check(CheckTiming.RemainingWindow(delayedStart) is >= 2850 and <= 3050);
long alreadyExpired = delayedStart - 4 * System.Diagnostics.Stopwatch.Frequency;
Check(CheckTiming.RemainingWindow(alreadyExpired) == 0);
cancelled.Set();
Check(CheckTiming.WaitWindow(cancelled, driver) == 0);
cancelled.Reset();
driver.Set();
Check(CheckTiming.WaitWindow(cancelled, driver) == 1);
Console.WriteLine("Audio check protocol, meter, independent window and early-stop primitives passed.");

var host = Environment.ProcessPath ?? throw new InvalidOperationException();
var childInfo = new ProcessStartInfo(host) { UseShellExecute = false, RedirectStandardOutput = true };
if (string.Equals(Path.GetFileNameWithoutExtension(host), "dotnet", StringComparison.OrdinalIgnoreCase))
    childInfo.ArgumentList.Add(typeof(Protocol).Assembly.Location);
childInfo.ArgumentList.Add("--stuck-sampler");
using var guardChild = Process.Start(childInfo) ?? throw new InvalidOperationException();
try
{
    var guardOutput = guardChild.StandardOutput.ReadToEndAsync();
    Check(guardChild.WaitForExit(5000));
    Check(guardChild.ExitCode == 3);
    Check((await guardOutput).Trim() == "capture-owner-release-simulated");
}
finally { if (!guardChild.HasExited) guardChild.Kill(entireProcessTree: true); }
Console.WriteLine("Stuck sampler subprocess used the production retirement decision: exited without ACK; no capture opened.");
