using System.Buffers;
using System.Formats.Tar;
using System.Security.Cryptography;
using ICSharpCode.SharpZipLib.BZip2;

static class EnglishModelExtract
{
    internal static void VerifiedArchive(Stream source, EnglishModelPins.Manifest pins,
        Func<string, Stream> create, CancellationToken ct)
    {
        // Caller must hold the same verified archive handle and validated directory.
        if (source.ReadByte() != 'B' || source.ReadByte() != 'Z' || source.ReadByte() != 'h' ||
            source.ReadByte() is < '1' or > '9') throw new SetupFailure("archive_invalid");
        source.Position = 0;
        var listed = new HashSet<string>(StringComparer.Ordinal);
        var installed = new HashSet<string>(StringComparer.Ordinal);
        var buffer = ArrayPool<byte>.Shared.Rent(64 * 1024);
        try
        {
            using var bzip = new BZip2InputStream(source) { IsStreamOwner = false };
            using var decoded = new LimitedRead(bzip, pins.ExpandedBytes, ct);
            using (var tar = new TarReader(decoded, leaveOpen: true))
            {
                while (tar.GetNextEntry(copyData: false) is { } entry)
                {
                    ct.ThrowIfCancellationRequested();
                    // No extracted path comes from archive text. The examined tar must
                    // still match the trusted shape and every type/name must be safe.
                    var path = entry.Name.TrimEnd('/');
                    var parts = path.Split('/');
                    if (parts.Length is < 1 or > 3 || parts[0] != pins.Id ||
                        parts.Any(p => p is "" or "." or ".." || p.Any(c => !(char.IsAsciiLetterOrDigit(c) || c is '.' or '-' or '_'))) ||
                        !listed.Add(path) || listed.Count > pins.EntryCount)
                        throw new SetupFailure("archive_invalid");
                    if (entry.EntryType == TarEntryType.Directory)
                    {
                        if (entry.Length != 0 || parts.Length == 2 && parts[1] != "test_wavs" ||
                            parts.Length == 3 || parts.Length == 2 && pins.Members.ContainsKey(parts[1]))
                            throw new SetupFailure("archive_invalid");
                        continue;
                    }
                    if (entry.EntryType is not (TarEntryType.RegularFile or TarEntryType.V7RegularFile) ||
                        parts.Length == 1 || parts.Length == 3 && parts[1] != "test_wavs")
                        throw new SetupFailure("archive_invalid");
                    if (parts.Length != 2 || !pins.Members.TryGetValue(parts[1], out var expected))
                        continue; // Drain only through the bounded decompressor, never write.
                    if (entry.Length != expected.Bytes || entry.DataStream is null || !installed.Add(parts[1]))
                        throw new SetupFailure("archive_invalid");
                    using var output = create(parts[1]);
                    using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
                    long written = 0;
                    while (true)
                    {
                        ct.ThrowIfCancellationRequested();
                        int read = entry.DataStream.Read(buffer, 0, 64 * 1024);
                        if (read == 0) break;
                        written += read;
                        if (written > expected.Bytes) throw new SetupFailure("archive_invalid");
                        hash.AppendData(buffer, 0, read);
                        output.Write(buffer, 0, read);
                    }
                    if (written != expected.Bytes || !Convert.ToHexString(hash.GetHashAndReset())
                        .Equals(expected.Hash, StringComparison.OrdinalIgnoreCase))
                        throw new SetupFailure("digest_mismatch");
                    if (output is FileStream durable) durable.Flush(flushToDisk: true);
                    else output.Flush();
                }
            }
            // TarReader has consumed end markers, but earlier end-of-archive
            // does not imply validated BZip2 CRC/EOF. Drain exact zero padding.
            int n;
            while ((n = decoded.Read(buffer, 0, 64 * 1024)) > 0)
                if (buffer.AsSpan(0, n).IndexOfAnyExcept((byte)0) >= 0) throw new SetupFailure("archive_invalid");
            if (listed.Count != pins.EntryCount || installed.Count != pins.Members.Count ||
                decoded.Count != pins.ExpandedBytes) throw new SetupFailure("archive_invalid");
        }
        catch (Exception ex) when (ex is BZip2Exception or InvalidDataException or EndOfStreamException)
        {
            throw new SetupFailure("archive_invalid");
        }
        finally { ArrayPool<byte>.Shared.Return(buffer); }
    }

    sealed class LimitedRead(Stream source, long limit, CancellationToken ct) : Stream
    {
        internal long Count { get; private set; }
        public override bool CanRead => true;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => throw new NotSupportedException();
        public override long Position { get => throw new NotSupportedException(); set => throw new NotSupportedException(); }
        public override int Read(byte[] buffer, int offset, int count)
        {
            ct.ThrowIfCancellationRequested();
            count = (int)Math.Min(Math.Min(count, 64 * 1024), limit - Count + 1);
            int n = source.Read(buffer, offset, count);
            Count += n;
            if (Count > limit) throw new SetupFailure("archive_invalid");
            return n;
        }
        public override void Flush() => throw new NotSupportedException();
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
    }
}
