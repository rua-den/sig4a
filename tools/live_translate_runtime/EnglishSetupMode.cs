using System.Text.Json;

static class EnglishSetupMode
{
    internal record Command(long Session, string Action);

    internal static Command Parse(ReadOnlyMemory<byte> raw, bool cancel)
    {
        using var json = JsonDocument.Parse(raw, new() { MaxDepth = 3 });
        if (json.RootElement.ValueKind != JsonValueKind.Object) throw new FormatException();
        var fields = new Dictionary<string, JsonElement>(StringComparer.Ordinal);
        foreach (var field in json.RootElement.EnumerateObject())
            if (!fields.TryAdd(field.Name, field.Value) || fields.Count > 4) throw new FormatException();
        if (fields.Count != (cancel ? 3 : 4) ||
            !fields.TryGetValue("v", out var version) || version.ValueKind != JsonValueKind.Number ||
            !version.TryGetInt32(out var v) || v != 1 || !fields.TryGetValue("type", out var kind) ||
            kind.ValueKind != JsonValueKind.String ||
            kind.GetString() != (cancel ? "model_setup_cancel" : "model_setup_start") ||
            !fields.TryGetValue("session", out var n) || n.ValueKind != JsonValueKind.Number ||
            !n.TryGetInt64(out var session) || session is < 1 or > Protocol.SessionLimit)
            throw new FormatException();
        if (cancel) return new Command(session, "cancel");
        if (!fields.TryGetValue("action", out var action) || action.ValueKind != JsonValueKind.String ||
            action.GetString() is not ("install" or "verify")) throw new FormatException();
        return new Command(session, action.GetString()!);
    }

    internal static async Task<int> Run(Stream input)
    {
        var reader = new Protocol.BoundedReader(input);
        Command setup;
        try
        {
            var first = await reader.Read();
            if (first is null) return 0; // No request, no package access or network.
            setup = Parse(first.Value, cancel: false);
        }
        catch (Exception ex) when (ex is FormatException or JsonException or IOException) { return 2; }

        using var stop = new CancellationTokenSource();
        using var total = new CancellationTokenSource(TimeSpan.FromMinutes(12));
        using var active = CancellationTokenSource.CreateLinkedTokenSource(stop.Token, total.Token);
        var run = Task.Run(Work);
        try
        {
            while (!run.IsCompleted)
            {
                var next = reader.Read().AsTask();
                if (await Task.WhenAny(run, next) == run) break;
                var message = await next;
                if (message is null) { stop.Cancel(); break; }
                if (Parse(message.Value, cancel: true).Session == setup.Session) { stop.Cancel(); break; }
            }
        }
        catch (Exception ex) when (ex is FormatException or JsonException or IOException) { stop.Cancel(); }
        return await run;

        void Stage(string value) => Program.Send(new { v = 1, type = "model_setup", session = setup.Session, state = value });
        void Failed(string code) => Program.Send(new { v = 1, type = "model_setup", session = setup.Session, state = "failed", code });
        async Task<int> Work()
        {
            try
            {
                await EnglishModelInstaller.Execute(setup.Action, Stage, received => Program.Send(new
                {
                    v = 1, type = "model_setup", session = setup.Session, state = "downloading",
                    received, total = EnglishModelPins.Pins.ArchiveBytes
                }), active.Token);
                Stage("ready");
                return 0;
            }
            catch (OperationCanceledException)
            {
                if (stop.IsCancellationRequested) { Stage("cancelled"); return 0; }
                Failed("timeout");
                return 1;
            }
            catch (Exception ex)
            {
                Failed(EnglishModelInstaller.ErrorCode(ex));
                return 1;
            }
        }
    }
}
