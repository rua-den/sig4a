using System.Security.Cryptography;
using System.Text.Json;

static class EnglishModelPins
{
    internal static readonly Uri Origin = new("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-2023-06-26.tar.bz2");
    internal const string HandoffHost = "release-assets.githubusercontent.com";
    internal const string Stem = "epoch-99-avg-1-chunk-16-left-128";
    internal const string Tokens = "tokens.txt";
    internal const string Encoder = $"encoder-{Stem}.int8.onnx";
    internal const string Decoder = $"decoder-{Stem}.onnx";
    internal const string Joiner = $"joiner-{Stem}.int8.onnx";
    internal static readonly string[] ModelFiles = [Tokens, Encoder, Decoder, Joiner];
    internal static readonly Manifest Pins = ReadManifest();
    internal record Member(long Bytes, string Hash);
    internal record Manifest(string Id, long ArchiveBytes, string ArchiveHash, long ExpandedBytes,
        int EntryCount, long MinimumFree, IReadOnlyDictionary<string, Member> Members);

    static Manifest ReadManifest()
    {
        using var resource = typeof(EnglishModelPins).Assembly.GetManifestResourceStream("Sig4a.EnglishAssets")!;
        using var json = JsonDocument.Parse(resource);
        var doc = json.RootElement;
        var sizes = doc.GetProperty("member_bytes");
        var files = doc.GetProperty("files").EnumerateObject().ToDictionary(x => x.Name,
            x => new Member(sizes.GetProperty(x.Name).GetInt64(), x.Value.GetString()!), StringComparer.Ordinal);
        return new(doc.GetProperty("id").GetString()!, doc.GetProperty("archive_bytes").GetInt64(),
            doc.GetProperty("archive_sha256").GetString()!, doc.GetProperty("expanded_bytes").GetInt64(),
            doc.GetProperty("tar_entries").GetInt32(), doc.GetProperty("min_free_bytes").GetInt64(), files);
    }

    internal static bool Matches(Stream stream, Member member, CancellationToken stop = default)
    {
        if (stream.Length != member.Bytes) return false;
        using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        var buffer = new byte[128 * 1024];
        while (true)
        {
            stop.ThrowIfCancellationRequested();
            int count = stream.Read(buffer, 0, buffer.Length);
            if (count == 0) break;
            hash.AppendData(buffer, 0, count);
        }
        stop.ThrowIfCancellationRequested();
        return Convert.ToHexString(hash.GetHashAndReset()).Equals(member.Hash, StringComparison.OrdinalIgnoreCase);
    }
}

sealed class SetupFailure(string code) : Exception(code)
{
    internal string Code { get; } = code;
}
