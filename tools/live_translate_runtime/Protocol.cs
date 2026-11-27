using System.Text.Encodings.Web;
using System.Text.Json;

static class Protocol
{
    internal const int LineLimit = 16_384;
    internal const long SessionLimit = 9_007_199_254_740_991L;
    internal static readonly JsonSerializerOptions Serialization = new() { Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping };
    internal record Command(string Type, long Session = 0, string? Source = null, string? Device = null, bool Translate = false);

    internal static Command Parse(ReadOnlyMemory<byte> raw)
    {
        using var json = JsonDocument.Parse(raw, new() { MaxDepth = 3 });
        if (json.RootElement.ValueKind != JsonValueKind.Object) throw new FormatException();
        var fields = new Dictionary<string, JsonElement>(StringComparer.Ordinal);
        foreach (var field in json.RootElement.EnumerateObject())
            if (!fields.TryAdd(field.Name, field.Value) || fields.Count > 6) throw new FormatException();
        if (!fields.TryGetValue("v", out var version) || version.ValueKind != JsonValueKind.Number ||
            !version.TryGetInt32(out var v) || v != 1 || !fields.TryGetValue("type", out var kind) ||
            kind.ValueKind != JsonValueKind.String) throw new FormatException();
        var type = kind.GetString();
        if (type is "list" or "quit" && fields.Count == 2) return new(type);
        if (type is not ("start" or "stop") || !fields.TryGetValue("session", out var id) ||
            id.ValueKind != JsonValueKind.Number || !id.TryGetInt64(out var session) ||
            session is < 1 or > SessionLimit) throw new FormatException();
        if (type == "stop" && fields.Count == 3) return new(type, session);
        if (type != "start" || fields.Count != 6 ||
            !fields.TryGetValue("source", out var src) || src.ValueKind != JsonValueKind.String ||
            !fields.TryGetValue("device_id", out var selected) ||
            !fields.TryGetValue("translate", out var trans) ||
            trans.ValueKind is not (JsonValueKind.True or JsonValueKind.False)) throw new FormatException();
        var source = src.GetString();
        if (source is not ("playback" or "mic")) throw new FormatException();
        string? device = selected.ValueKind == JsonValueKind.String ? selected.GetString() : null;
        if (device is null && (selected.ValueKind != JsonValueKind.Null || source == "mic") ||
            device is not null && (device.Length is < 1 or > 512 || device.Any(char.IsControl)))
            throw new FormatException();
        return new(type, session, source, device, trans.GetBoolean());
    }

    internal sealed class BoundedReader(Stream input)
    {
        readonly byte[] bytes = new byte[4096];
        readonly byte[] line = new byte[LineLimit];
        int cursor, used;

        // Views are consumed synchronously before the next read.
        internal async ValueTask<ReadOnlyMemory<byte>?> Read()
        {
            int n = 0;
            while (true)
            {
                if (cursor == used)
                {
                    used = await input.ReadAsync(bytes).ConfigureAwait(false);
                    cursor = 0;
                    if (used == 0) return n == 0 ? null : throw new FormatException();
                }
                byte c = bytes[cursor++];
                if (c == 10) return line.AsMemory(0, n);
                if (n == LineLimit) throw new FormatException();
                line[n++] = c;
            }
        }
    }
}
