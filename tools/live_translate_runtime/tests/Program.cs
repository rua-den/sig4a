using System.Text;
using System.Text.Json;

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
