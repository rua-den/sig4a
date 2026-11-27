using System.Buffers.Binary;
using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text.Json;
using System.Threading.Channels;

sealed class OfflineTranslation(Action<long, string, string> translated, Action unavailable, CancellationToken sessionToken)
{
    const string Engine = "sig4a-hymt";
    const string Model = "HY-MT1.5-1.8B-Q4_K_M.gguf";
    const int TcpStateEstablished = 5;
    const int TcpOwnerPidAll = 5;
    const int AddressFamilyInet = 2;
    const int EntrySize = 24;
    readonly Channel<(long Seq, string En)> queue = Channel.CreateBounded<(long, string)>(
        new BoundedChannelOptions(1) { SingleReader = true, SingleWriter = true, FullMode = BoundedChannelFullMode.DropOldest });
    internal bool Released { get; private set; } = true;
    internal event Action? InferenceDispatched;

    internal void Enqueue(long seq, string en) => queue.Writer.TryWrite((seq, en));

    static async Task<string> VerifyAssets(CancellationToken ct)
    {
        string path = Path.Combine(AppContext.BaseDirectory, "models", "hymt");
        string runtime = Path.Combine(path, "runtime");
        foreach (string dir in new[] { Path.GetDirectoryName(path)!, path, runtime })
            if ((File.GetAttributes(dir) & FileAttributes.ReparsePoint) != 0) throw new IOException();
        var acceptance = new FileInfo(Path.Combine(path, "publisher-terms.accepted"));
        if (!acceptance.Exists || acceptance.Length != 64 ||
            (acceptance.Attributes & FileAttributes.ReparsePoint) != 0 ||
            await File.ReadAllTextAsync(acceptance.FullName, ct) != "d7d9db858500ac9073f4b5decef8e208454357226f535f65079ce4376047569f")
            throw new IOException();
        using var pins = typeof(OfflineTranslation).Assembly.GetManifestResourceStream("Sig4a.TranslationAssets")
            ?? throw new IOException();
        using var json = await JsonDocument.ParseAsync(pins, cancellationToken: ct);
        var hashes = json.RootElement.GetProperty("files").EnumerateObject().ToDictionary(
            member => member.Name, member => member.Value.GetString()!, StringComparer.OrdinalIgnoreCase);
        var files = Directory.GetFiles(runtime);
        if (files.Length != hashes.Count || Directory.EnumerateDirectories(runtime).Any()) throw new IOException();
        foreach (var file in files)
        {
            if (!hashes.TryGetValue(Path.GetFileName(file), out var sha)) throw new IOException();
            await Verify(file, sha, 32_000_000, ct);
        }
        await Verify(Path.Combine(path, Model), json.RootElement.GetProperty("model_sha256").GetString()!,
            1_133_080_512, ct);
        return path;
    }

    static async Task Verify(string path, string expected, long limit, CancellationToken ct)
    {
        var file = new FileInfo(path);
        if (!file.Exists || file.Length < 100 || file.Length > limit ||
            (file.Attributes & FileAttributes.ReparsePoint) != 0) throw new IOException();
        using var data = file.OpenRead();
        var sha = await SHA256.HashDataAsync(data, ct);
        if (!Convert.ToHexString(sha).Equals(expected, StringComparison.OrdinalIgnoreCase)) throw new IOException();
    }

    internal async Task Run()
    {
        Process? process = null;
        Task? consumeOut = null, consumeErr = null;
        try
        {
            var path = await VerifyAssets(sessionToken);
            int port;
            using (var reserve = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp))
            {
                reserve.Bind(new IPEndPoint(IPAddress.Loopback, 0));
                port = ((IPEndPoint)reserve.LocalEndPoint!).Port;
            }
            var cfg = new ProcessStartInfo(Path.Combine(path, "runtime", "llama-server.exe"))
            {
                WorkingDirectory = Path.Combine(path, "runtime"),
                UseShellExecute = false, RedirectStandardOutput = true, RedirectStandardError = true,
                RedirectStandardInput = true, CreateNoWindow = true
            };
            foreach (string key in cfg.Environment.Keys.ToArray().Where(k =>
                         k.StartsWith("LLAMA_", StringComparison.OrdinalIgnoreCase) ||
                         k.StartsWith("GGML_", StringComparison.OrdinalIgnoreCase) ||
                         k.EndsWith("_PROXY", StringComparison.OrdinalIgnoreCase))) cfg.Environment.Remove(key);
            // The pinned server reads LLAMA_API_KEY in b12776. Never put the
            // ephemeral credential in argv, stdout, config files or user settings.
            string secret = Convert.ToHexString(RandomNumberGenerator.GetBytes(32));
            cfg.Environment["LLAMA_API_KEY"] = secret;
            cfg.Environment["PATH"] = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows), "System32");
            foreach (var arg in new[] { "--host", "127.0.0.1", "--port", port.ToString(System.Globalization.CultureInfo.InvariantCulture),
                         "--alias", Engine, "-ngl", "0", "-t", "4", "-c", "2048", "-np", "1", "--no-webui", "--no-ui-mcp-proxy",
                         "--offline", "--no-slots", "--no-cache-prompt", "--cache-ram", "0",
                         "--no-context-shift", "--log-disable", "--timeout", "6",
                         "-m", Path.Combine(path, Model) })
                cfg.ArgumentList.Add(arg);
            sessionToken.ThrowIfCancellationRequested();
            process = Process.Start(cfg) ?? throw new IOException();
            Released = false;
            process.StandardInput.Close();
            consumeOut = process.StandardOutput.BaseStream.CopyToAsync(Stream.Null);
            consumeErr = process.StandardError.BaseStream.CopyToAsync(Stream.Null);

            using var transport = new SocketsHttpHandler
            {
                UseProxy = false, AllowAutoRedirect = false, PooledConnectionLifetime = TimeSpan.Zero,
                ConnectTimeout = TimeSpan.FromSeconds(1),
                ConnectCallback = (ctx, ct) =>
                {
                    if (ctx.DnsEndPoint.Host != "127.0.0.1" || ctx.DnsEndPoint.Port != port)
                        throw new IOException();
                    return OwnedSocket(process, port, ct);
                }
            };
            using var client = new HttpClient(transport) { BaseAddress = new Uri($"http://127.0.0.1:{port}"), Timeout = Timeout.InfiniteTimeSpan };
            using (var ready = CancellationTokenSource.CreateLinkedTokenSource(sessionToken))
            {
                ready.CancelAfter(TimeSpan.FromSeconds(45));
                while (true)
                {
                    ready.Token.ThrowIfCancellationRequested();
                    if (process.HasExited) throw new IOException();
                    using var attempt = CancellationTokenSource.CreateLinkedTokenSource(ready.Token);
                    attempt.CancelAfter(TimeSpan.FromSeconds(2));
                    try
                    {
                        using var result = await client.GetAsync("/health", HttpCompletionOption.ResponseHeadersRead, attempt.Token);
                        if (result.IsSuccessStatusCode) break;
                    }
                    catch (HttpRequestException) { }
                    catch (OperationCanceledException) when (!ready.IsCancellationRequested) { }
                    await Task.Delay(300, ready.Token);
                }
            }

            using (var check = CancellationTokenSource.CreateLinkedTokenSource(sessionToken))
            {
                check.CancelAfter(TimeSpan.FromSeconds(3));
                // Test the actual child, without including any microphone text.
                // Admit content only if an unauthenticated inference request is denied.
                using var denied = await client.PostAsync("/v1/chat/completions",
                    JsonContent(new { messages = Array.Empty<object>() }), check.Token);
                if (denied.StatusCode is not (HttpStatusCode.Unauthorized or HttpStatusCode.Forbidden))
                    throw new IOException();
                client.DefaultRequestHeaders.Authorization = new("Bearer", secret);
                foreach (var route in new[] { "/slots", "/metrics" })
                {
                    using var probe = await client.GetAsync(route, HttpCompletionOption.ResponseHeadersRead, check.Token);
                    if (probe.IsSuccessStatusCode) throw new IOException();
                }
            }

            await foreach (var job in queue.Reader.ReadAllAsync(sessionToken))
            {
                using var deadline = CancellationTokenSource.CreateLinkedTokenSource(sessionToken);
                deadline.CancelAfter(TimeSpan.FromSeconds(5));
                using var request = new HttpRequestMessage(HttpMethod.Post, "/v1/chat/completions")
                {
                    Content = JsonContent(new
                    {
                        model = Engine, messages = new[] { new { role = "user", content =
                            "Translate the following segment into Vietnamese, without additional explanation.\n\n" + job.En } },
                        stream = false, max_tokens = 120, top_k = 20, top_p = .6, temperature = .7,
                        repeat_penalty = 1.05, seed = 7301, cache_prompt = false
                    })
                };
                var pending = client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, deadline.Token);
                InferenceDispatched?.Invoke();
                using var result = await pending;
                result.EnsureSuccessStatusCode();
                using var body = await result.Content.ReadAsStreamAsync(deadline.Token);
                var bytes = new byte[32_769];
                int got = await body.ReadAtLeastAsync(bytes, bytes.Length, false, deadline.Token);
                if (got == bytes.Length) throw new IOException();
                using var json = JsonDocument.Parse(bytes.AsMemory(0, got), new() { MaxDepth = 12 });
                var choice = json.RootElement.GetProperty("choices")[0];
                if (choice.GetProperty("finish_reason").GetString() != "stop") continue;
                if (!sessionToken.IsCancellationRequested && choice.GetProperty("message").GetProperty("content").GetString() is { } vi && vi.Trim().Length != 0)
                    translated(job.Seq, job.En, vi);
            }
        }
        catch (OperationCanceledException) when (sessionToken.IsCancellationRequested) { }
        catch (Exception) { unavailable(); }
        finally
        {
            if (process is not null)
            {
                try
                {
                    if (!process.HasExited) process.Kill(entireProcessTree: true);
                    using var reaper = new CancellationTokenSource(1200);
                    await process.WaitForExitAsync(reaper.Token);
                    Released = true;
                    if (consumeOut is not null && consumeErr is not null)
                        await Task.WhenAny(Task.WhenAll(consumeOut, consumeErr), Task.Delay(100));
                }
                catch (Exception) { Released = false; }
                finally { process.Dispose(); }
            }
        }
    }

    static ByteArrayContent JsonContent<T>(T value)
    {
        var content = new ByteArrayContent(JsonSerializer.SerializeToUtf8Bytes(value, Protocol.Serialization));
        content.Headers.ContentType = new("application/json");
        return content;
    }

    internal static async ValueTask<Stream> OwnedSocket(Process process, int port, CancellationToken ct)
    {
        var socket = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp);
        try
        {
            await socket.ConnectAsync(new IPEndPoint(IPAddress.Loopback, port), ct);
            var localPort = ((IPEndPoint)socket.LocalEndPoint!).Port;
            // Verify the already-connected peer on *this socket*, not merely a listener
            // snapshot taken before connecting. No HTTP/text is written if proof fails.
            if (process.HasExited || !OwnsConnectedPeer(process.Id, port, localPort) || process.HasExited)
                throw new IOException();
            return new NetworkStream(socket, ownsSocket: true);
        }
        catch { socket.Dispose(); throw; }
    }

    [DllImport("iphlpapi.dll", ExactSpelling = true)]
    static extern uint GetExtendedTcpTable(IntPtr table, ref int size,
        [MarshalAs(UnmanagedType.Bool)] bool order, int family, int tableClass, uint reserved);

    static bool OwnsConnectedPeer(int pid, int port, int localPort)
    {
        const int limit = 8 * 1024 * 1024;
        int size = 0;
        if (GetExtendedTcpTable(IntPtr.Zero, ref size, false, AddressFamilyInet, TcpOwnerPidAll, 0) != 122 ||
            size is < 4 or > limit) return false;
        var memory = Marshal.AllocHGlobal(size);
        try
        {
            if (GetExtendedTcpTable(memory, ref size, false, AddressFamilyInet, TcpOwnerPidAll, 0) != 0)
                return false;
            int count = Marshal.ReadInt32(memory);
            if (count < 0 || count > (size - 4) / EntrySize) return false;
            for (int i = 0; i < count; i++)
            {
                var row = memory + 4 + i * EntrySize;
                if (Marshal.ReadInt32(row) == TcpStateEstablished &&
                    Marshal.ReadInt32(row, 4) == 0x0100007F &&
                    Marshal.ReadInt32(row, 12) == 0x0100007F &&
                    BinaryPrimitives.ReverseEndianness(unchecked((ushort)Marshal.ReadInt32(row, 8))) == port &&
                    BinaryPrimitives.ReverseEndianness(unchecked((ushort)Marshal.ReadInt32(row, 16))) == localPort &&
                    Marshal.ReadInt32(row, 20) == pid) return true;
            }
            return false;
        }
        finally { Marshal.FreeHGlobal(memory); }
    }
}
