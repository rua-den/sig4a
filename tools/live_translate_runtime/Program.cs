using System.Text.Json;
using NAudio.CoreAudioApi;

static class Program
{
    static readonly Stream Output = Console.OpenStandardOutput();
    static readonly object Gate = new();
    static readonly byte[] Newline = [10];
    static Session? Active;
    static CheckSession? Checking;
    static bool lastWasCheck;
    static WindowsModelPaths.PackageLease? packageLease;
    static long lastId;

    static bool AcquirePackage(long session)
    {
        try
        {
            packageLease = WindowsModelPaths.Acquire(EnglishModelInstaller.Root);
            return true;
        }
        catch (Exception ex)
        {
            var code = EnglishModelInstaller.ErrorCode(ex);
            Error(session, code == "busy" ? "busy" : "backend_missing");
            return false;
        }
    }
    static void ReleasePackage()
    {
        packageLease?.Dispose();
        packageLease = null;
    }
    static bool closing;

    internal static void Send<T>(T message)
    {
        byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(message, Protocol.Serialization);
        if (bytes.Length > Protocol.LineLimit) throw new InvalidOperationException();
        Output.Write(bytes);
        Output.Write(Newline);
        Output.Flush();
    }

    internal static void Error(long session, string code) => Send(new { v = 1, type = "error", session, code });
    internal static void Status(long session, string state) => Send(new { v = 1, type = "status", session, state });

    static void DeviceList()
    {
        var devices = new List<object>();
        try
        {
            using var host = new MMDeviceEnumerator();
            foreach (var flow in new[] { DataFlow.Render, DataFlow.Capture })
            {
                string? defaultId = null;
                try { using var endpoint = host.GetDefaultAudioEndpoint(flow, Role.Multimedia); defaultId = endpoint.ID; }
                catch (Exception) { /* no default for this flow */ }
                var source = flow == DataFlow.Render ? "playback" : "mic";
                foreach (var endpoint in host.EnumerateAudioEndPoints(flow, DeviceState.Active))
                {
                    using (endpoint)
                    {
                        if (devices.Count == 32) break;
                        string id = endpoint.ID;
                        if (id.Length is < 1 or > 512 || id.Any(char.IsControl)) continue;
                        var name = TextCap(endpoint.FriendlyName, 128);
                        devices.Add(new { id, name, source, is_default = id == defaultId });
                        // UTF-8 string length and escaping may exceed the aggregate wire budget.
                        if (JsonSerializer.SerializeToUtf8Bytes(new { v = 1, type = "devices", devices },
                            Protocol.Serialization).Length > Protocol.LineLimit) devices.RemoveAt(devices.Count - 1);
                    }
                }
            }
            Send(new { v = 1, type = "devices", devices });
        }
        catch (Exception) { Error(0, "device_missing"); }
    }

    internal static string TextCap(string text, int max)
    {
        // Captions/name data are plain accessible text, not embedded controls.
        text = new string(text.Where(c => !char.IsControl(c)).ToArray());
        int length = Math.Min(text.Length, max);
        if (length > 0 && char.IsHighSurrogate(text[length - 1])) length--;
        return text[..length];
    }

    internal static bool Started(Session source)
    {
        lock (Gate)
        {
            if (closing || Active != source || source.Cancelled) return false;
            Status(source.Id, "started");
            return true;
        }
    }

    internal static void Caption(Session source, string text, long seq)
    {
        text = TextCap(text.Trim(), 2048);
        if (text.Length == 0) return;
        lock (Gate)
        {
            if (closing || Active != source || source.Cancelled) return;
            Send(new { v = 1, type = "caption", session = source.Id, en = text, vi = (string?)null, seq });
        }
    }

    internal static void Translated(Session source, long seq, string en, string text)
    {
        text = TextCap(text.Trim(), 2048);
        if (text.Length == 0) return;
        lock (Gate)
        {
            if (closing || Active != source || source.Cancelled || source.LatestSequence != seq) return;
            // Size accounting must include both captions. Keep the EN text stable
            // across correlated revisions even if Unicode escaping expands the response.
            while (text.Length > 0)
            {
                var message = new { v = 1, type = "caption", session = source.Id, en, vi = text, seq };
                if (JsonSerializer.SerializeToUtf8Bytes(message, Protocol.Serialization).Length <= Protocol.LineLimit)
                {
                    Send(message);
                    break;
                }
                text = TextCap(text, text.Length / 2);
            }
        }
    }

    internal static void TranslatorUnavailable(Session source)
    {
        lock (Gate)
        {
            if (!closing && Active == source && !source.Cancelled) Error(source.Id, "translation_unavailable");
        }
    }

    internal static void Finished(Session source, string? failure, bool released, bool stopping)
    {
        lock (Gate)
        {
            if (Active != source) return;
            Active = null;
            if (released) ReleasePackage();
            if (closing) return;
            if (!released)
            {
                // Never acknowledge release if native Stop or Dispose failed.
                Error(source.Id, "internal");
                Environment.Exit(3);
            }
            if (failure is not null && !stopping) Error(source.Id, failure);
            Status(source.Id, "stopped");
        }
    }

    internal static bool CheckStarted(CheckSession source)
    {
        lock (Gate)
        {
            if (closing || Checking != source || !source.CanReport) return false;
            Send(new { v = 1, type = "check_status", session = source.Id, state = "started" });
            return true;
        }
    }

    internal static void CheckMeter(CheckSession source, int level, bool signal)
    {
        lock (Gate)
        {
            if (!closing && Checking == source && source.CanReport)
                Send(new { v = 1, type = "check_meter", session = source.Id, level, signal });
        }
    }

    internal static void CheckSample(CheckSession source, string candidate)
    {
        string en = TextCap(candidate.Trim(), 512);
        lock (Gate)
        {
            if (!closing && Checking == source && source.CanReport && en.Length > 0)
                Send(new { v = 1, type = "check_sample", session = source.Id, en });
        }
    }

    internal static void CheckModel(CheckSession source, bool ready)
    {
        lock (Gate)
        {
            if (!closing && Checking == source && source.CanReport)
                Send(new { v = 1, type = "check_model", session = source.Id,
                    state = ready ? "ready" : "unavailable" });
        }
    }

    internal static void CheckFinished(CheckSession source, bool released, bool completed, string? error, bool signal)
    {
        lock (Gate)
        {
            if (Checking != source) return;
            if (!released) Environment.Exit(3);
            Checking = null;
            ReleasePackage();
            if (closing) return;
            if (error is not null) Error(source.Id, error);
            if (completed) Send(new { v = 1, type = "check_meter", session = source.Id, level = 0, signal });
            Send(new { v = 1, type = "check_status", session = source.Id, state = "stopped", completed });
        }
    }

    [MTAThread]
    static async Task<int> Main(string[] args)
    {
        if (!OperatingSystem.IsWindows()) return 2;
        if (args is ["--english-model-setup"]) return await EnglishSetupMode.Run(Console.OpenStandardInput());
        if (args.Length != 0) return 2;
        var input = new Protocol.BoundedReader(Console.OpenStandardInput());
        var exit = 0;
        try
        {
            while (await input.Read() is { } raw)
            {
                var cmd = Protocol.Parse(raw);
                if (cmd.Type == "quit") break;
                lock (Gate)
                {
                    switch (cmd.Type)
                    {
                        case "list":
                            DeviceList();
                            break;
                        case "start":
                            if (cmd.Session <= lastId) break;
                            if (Active is not null || Checking is not null) { Error(cmd.Session, "busy"); break; }
                            if (!AcquirePackage(cmd.Session)) break;
                            lastId = cmd.Session;
                            lastWasCheck = false;
                            Active = new Session(cmd.Session, cmd.Source!, cmd.Device, cmd.Translate, packageLease!.Root);
                            Active.Work = Task.Run(Active.Run);
                            break;
                        case "check_start":
                            if (cmd.Session <= lastId) break;
                            if (Active is not null || Checking is not null) { Error(cmd.Session, "busy"); break; }
                            if (!AcquirePackage(cmd.Session)) break;
                            lastId = cmd.Session;
                            lastWasCheck = true;
                            var check = new CheckSession(cmd.Session, cmd.Source!, cmd.Device, packageLease!.Root);
                            Checking = check;
                            check.Work = check.Run();
                            break;
                        case "check_stop":
                            if (cmd.Session == lastId && lastWasCheck)
                            {
                                if (Checking is not null) Checking.Stop();
                                else Send(new { v = 1, type = "check_status", session = cmd.Session, state = "stopped", completed = false });
                            }
                            break;
                        case "stop":
                            if (cmd.Session == lastId && !lastWasCheck)
                            {
                                if (Active is not null) Active.Stop();
                                else Status(cmd.Session, "stopped");
                            }
                            break;
                    }
                }
            }
        }
        catch (Exception e) when (e is FormatException or JsonException or InvalidOperationException or IOException)
        {
            exit = 2;
            lock (Gate)
            {
                closing = true;
                Active?.Stop();
                Checking?.Stop();
            }
        }
        Task? pending;
        lock (Gate)
        {
            closing = true;
            pending = Active?.Work ?? Checking?.Work;
            Active?.Stop();
            Checking?.Stop();
        }
        if (pending is not null && await Task.WhenAny(pending, Task.Delay(4_000)) != pending)
            return 3; // Parent watchdog also reaps this process on deadline.
        if (exit != 0)
        {
            try { Error(0, "protocol_error"); }
            catch (IOException) { /* stdin owner disappeared */ }
        }
        return exit;
    }
}
