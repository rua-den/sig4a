using System.Buffers.Binary;
using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using NAudio.Wave;

if (!OperatingSystem.IsWindows() || args.Length is < 1 or > 2 ||
    args.Length == 2 && args[1] != "--vi") throw new InvalidOperationException();
using (var model = Session.LoadModel())
using (var stream = model.CreateStream())
using (var wave = new WaveFileReader(args[0]))
{
    var rate = wave.WaveFormat.SampleRate;
    if (wave.WaveFormat.Encoding != WaveFormatEncoding.Pcm || wave.WaveFormat.Channels != 1 ||
        wave.WaveFormat.BitsPerSample != 16 || rate != 16000 || wave.TotalTime.TotalSeconds > 12)
        throw new InvalidDataException("Expected bounded 16 kHz PCM synthetic fixture.");
    var chunk = new byte[rate / 20 * 2];
    var recognized = new List<string>();
    for (int n = 0; ;)
    {
        n = wave.Read(chunk);
        if (n == 0) break;
        var values = new float[n / 2];
        for (int i = 0; i < values.Length; i++)
            values[i] = BinaryPrimitives.ReadInt16LittleEndian(chunk.AsSpan(i * 2, 2)) / 32768f;
        Feed(values);
    }
    var silent = new float[rate / 20];
    for (int i = 0; i < 60; i++) Feed(silent);
    var text = string.Join(' ', recognized);
    if (!text.Contains("MEETING", StringComparison.OrdinalIgnoreCase) ||
        !text.Contains("WINDOW", StringComparison.OrdinalIgnoreCase))
        throw new InvalidOperationException("The synthetic ASR phrase did not reach a plausible endpoint.");

    void Feed(float[] wave)
    {
        stream.AcceptWaveform(rate, wave);
        while (model.IsReady(stream)) model.Decode(stream);
        if (!model.IsEndpoint(stream)) return;
        recognized.Add(model.GetResult(stream).Text);
        model.Reset(stream);
    }
}
Console.WriteLine("PASS: exact production Sherpa config, pinned real assets, synthetic PCM endpoint; no endpoints opened.");

using (var self = Process.GetCurrentProcess())
using (var other = Process.Start(new ProcessStartInfo(
    Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows), "System32", "WindowsPowerShell",
                 "v1.0", "powershell.exe"),
    "-NoProfile -NonInteractive -Command Start-Sleep -Seconds 20") { CreateNoWindow = true, UseShellExecute = false })!)
using (var clock = new CancellationTokenSource(TimeSpan.FromSeconds(10)))
{
    using var listener = new TcpListener(IPAddress.Loopback, 0);
    listener.Start();
    int port = ((IPEndPoint)listener.LocalEndpoint).Port;
    try
    {
        var accept = listener.AcceptSocketAsync(clock.Token);
        await using var trusted = await OfflineTranslation.OwnedSocket(self, port, clock.Token);
        using var samePeer = await accept;
        accept = listener.AcceptSocketAsync(clock.Token);
        try
        {
            await using var rejected = await OfflineTranslation.OwnedSocket(other, port, clock.Token);
            throw new InvalidOperationException("Unowned loopback peer was accepted.");
        }
        catch (IOException) { }
        using var thirdParty = await accept;
        if (thirdParty.Available != 0) throw new InvalidOperationException("Rejected peer got payload bytes.");
    }
    finally { if (!other.HasExited) other.Kill(entireProcessTree: true); }
}
Console.WriteLine("PASS: exact production connected-owner verification accepts and rejects the expected PIDs.");

if (args.Length == 2)
{
    using var session = new CancellationTokenSource(TimeSpan.FromSeconds(100));
    var translated = new TaskCompletionSource<string>(TaskCreationOptions.RunContinuationsAsynchronously);
    const string phrase = "The meeting starts at three.";
    var dispatched = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
    int requestCount = 0, lateResults = 0;
    var mt = new OfflineTranslation((id, en, vi) =>
    {
        if (id == 1 && en == phrase) translated.TrySetResult(vi);
        if (id == 2 && session.IsCancellationRequested) Interlocked.Increment(ref lateResults);
    }, () => translated.TrySetException(new InvalidOperationException("Native local translation was unavailable.")), session.Token);
    mt.InferenceDispatched += () => { if (Interlocked.Increment(ref requestCount) == 2) dispatched.TrySetResult(); };
    mt.Enqueue(1, phrase);
    var work = Task.Run(mt.Run);
    try
    {
        var vi = await translated.Task.WaitAsync(session.Token);
        if (vi.Equals(phrase, StringComparison.OrdinalIgnoreCase) || vi.Length < 6)
            throw new InvalidOperationException("Unexpected smoke translation result.");
        mt.Enqueue(2, string.Concat(Enumerable.Repeat(phrase + " ", 50)));
        await dispatched.Task.WaitAsync(session.Token);
    }
    finally
    {
        session.Cancel();
        await work.WaitAsync(TimeSpan.FromMilliseconds(1800));
        if (lateResults != 0 || !mt.Released) throw new InvalidOperationException("Translation process was not reaped.");
    }
    Console.WriteLine("PASS: exact verified local HY-MT rejected unauthenticated inference, translated synthetic input, and reaped an active request.");
}
