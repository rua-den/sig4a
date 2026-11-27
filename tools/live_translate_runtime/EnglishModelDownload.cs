using System.Buffers;
using System.Diagnostics;
using System.Net;
using System.Net.Http;
using System.Security.Cryptography;

static class EnglishModelDownload
{
    internal static bool ValidHandoff(Uri? uri) => uri is { IsAbsoluteUri: true, Scheme: "https", Port: 443 }
        && uri.IdnHost.Equals(EnglishModelPins.HandoffHost, StringComparison.OrdinalIgnoreCase)
        && uri.UserInfo.Length == 0 && uri.Fragment.Length == 0;

    static bool Redirect(HttpStatusCode status) => (int)status is 301 or 302 or 303 or 307 or 308;

    internal static async Task Fetch(Stream archive, Action<long> progress, CancellationToken ct)
    {
        using var handler = new SocketsHttpHandler
        {
            AllowAutoRedirect = false, UseProxy = false, UseCookies = false,
            AutomaticDecompression = DecompressionMethods.None, Credentials = null,
            ConnectTimeout = TimeSpan.FromSeconds(20)
        };
        using var http = new HttpClient(handler) { Timeout = Timeout.InfiniteTimeSpan };
        await FetchVerified(http, archive, EnglishModelPins.Pins, progress, ct);
    }

    // Only the test assembly can inject transport; protocol has no destination override.
    internal static async Task FetchVerified(HttpClient http, Stream archive, EnglishModelPins.Manifest pins,
        Action<long> progress, CancellationToken ct)
    {
        var origin = EnglishModelPins.Origin;
        using var initial = await Get(origin);
        if (Redirect(initial.StatusCode))
        {
            Uri? destination = initial.Headers.Location;
            if (!ValidHandoff(destination)) throw new SetupFailure("network_unavailable");
            using var handoff = await Get(destination!);
            await Read(handoff);
        }
        else await Read(initial);

        async Task<HttpResponseMessage> Get(Uri uri)
        {
            using var waiting = CancellationTokenSource.CreateLinkedTokenSource(ct);
            waiting.CancelAfter(TimeSpan.FromSeconds(30));
            using var request = new HttpRequestMessage(HttpMethod.Get, uri);
            try
            {
                return await http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, waiting.Token);
            }
            catch (OperationCanceledException) when (!ct.IsCancellationRequested) { throw new SetupFailure("timeout"); }
            catch (HttpRequestException) { throw new SetupFailure("network_unavailable"); }
        }

        async Task Read(HttpResponseMessage response)
        {
            if (response.StatusCode != HttpStatusCode.OK || response.Content.Headers.ContentRange is not null ||
                response.Content.Headers.ContentEncoding.Count != 0)
                throw new SetupFailure("network_unavailable");
            if (response.Content.Headers.ContentLength is { } length && length != pins.ArchiveBytes)
                throw new SetupFailure("digest_mismatch");
            using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
            using var body = await response.Content.ReadAsStreamAsync(ct);
            var buf = ArrayPool<byte>.Shared.Rent(64 * 1024);
            try
            {
                long received = 0;
                var watch = Stopwatch.StartNew();
                progress(0);
                while (true)
                {
                    using var waiting = CancellationTokenSource.CreateLinkedTokenSource(ct);
                    waiting.CancelAfter(TimeSpan.FromSeconds(25));
                    int count;
                    try { count = await body.ReadAsync(buf.AsMemory(0, 64 * 1024), waiting.Token); }
                    catch (OperationCanceledException) when (!ct.IsCancellationRequested) { throw new SetupFailure("timeout"); }
                    catch (IOException) { throw new SetupFailure("network_unavailable"); }
                    if (count == 0) break;
                    received += count;
                    if (received > pins.ArchiveBytes) throw new SetupFailure("digest_mismatch");
                    hash.AppendData(buf, 0, count);
                    await archive.WriteAsync(buf.AsMemory(0, count), ct);
                    if (watch.ElapsedMilliseconds >= 300 || received == pins.ArchiveBytes)
                    {
                        progress(received);
                        watch.Restart();
                    }
                }
                await archive.FlushAsync(ct);
                if (received != pins.ArchiveBytes || !Convert.ToHexString(hash.GetHashAndReset())
                        .Equals(pins.ArchiveHash, StringComparison.OrdinalIgnoreCase))
                    throw new SetupFailure("digest_mismatch");
                archive.Position = 0;
            }
            finally { ArrayPool<byte>.Shared.Return(buf); }
        }
    }
}
