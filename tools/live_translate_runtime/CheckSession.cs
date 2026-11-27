using System.Buffers.Binary;
using System.Diagnostics;
using System.Runtime.InteropServices;
using NAudio.CoreAudioApi;
using NAudio.Wave;

sealed class CheckSession(long id, string source, string? deviceId, WindowsModelPaths.Pin packageRoot)
{
    const int SampleAfterMs = 2_400, SampleSeconds = 3;
    readonly CancellationTokenSource cancel = new(), cancelSample = new();
    readonly TaskCompletionSource live = new(TaskCreationOptions.RunContinuationsAsynchronously);
    readonly TaskCompletionSource<Outcome> done = new(TaskCreationOptions.RunContinuationsAsynchronously);
    readonly object samplesLock = new(), sampleLaunchLock = new();
    readonly TaskCompletionSource safeToReport = new(TaskCreationOptions.RunContinuationsAsynchronously);
    Task? sampleWork;
    float[]? samples;
    int collected, sampleRate, level, signal, captureClosing;
    bool acceptSamples = true;
    long firstCapture, nativeStartInvoked, latestPacket;
    internal long Id => id;
    internal bool CanReport => !cancel.IsCancellationRequested && Volatile.Read(ref captureClosing) == 0 && !done.Task.IsCompleted;
    internal Task Work { get; set; } = Task.CompletedTask;
    internal void Stop() { cancel.Cancel(); cancelSample.Cancel(); }
    sealed record Outcome(bool Released, bool Complete, string? Error);

    internal async Task Run()
    {
        // Neither this process's stdout/backpressure nor ASR scheduling can keep
        // the audio source open: capture shutdown and watchdog are native threads.
        var sourceThread = new Thread(Capture) { IsBackground = true };
        sourceThread.SetApartmentState(ApartmentState.MTA);
        var watchdog = new Thread(Guard) { IsBackground = true };
        watchdog.Start();
        sourceThread.Start();
        if (await Task.WhenAny(live.Task, done.Task) == live.Task && Program.CheckStarted(this))
        {
            lock (sampleLaunchLock)
            {
                if (CanReport) sampleWork = Task.Run(Sample);
            }
            while (CanReport)
            {
                var tick = Interlocked.Read(ref latestPacket);
                int meter = tick != 0 && Stopwatch.GetElapsedTime(tick).TotalMilliseconds < 350
                    ? Volatile.Read(ref level) : 0;
                Program.CheckMeter(this, meter, Volatile.Read(ref signal) != 0);
                if (await Task.WhenAny(done.Task, Task.Delay(CheckTiming.MeterMs, cancel.Token)) == done.Task) break;
            }
        }
        var result = await done.Task;
        await safeToReport.Task;
        DropSamples();
        Program.CheckFinished(this, result.Released, result.Complete, result.Error, Volatile.Read(ref signal) != 0);
    }

    void Guard()
    {
        var first = Task.WaitAny([live.Task, done.Task], CheckTiming.StartupMs);
        if (first == -1 || (first == 0 &&
            !done.Task.Wait(CheckTiming.RemainingWindow(nativeStartInvoked) + CheckTiming.ReleaseMs + 150)))
            Environment.Exit(3); // No ACK after unproven release; independent of stdout/ASR.

        // Capture is already stopped. A wedged native decoder may own a scratch
        // buffer, so never acknowledge completion or reuse this process for a
        // normal session until that worker has exited. Dashboard Job reaps it.
        Task? sampler;
        lock (sampleLaunchLock) sampler = sampleWork;
        CheckTiming.RetireSampleOrExit(sampler);
        safeToReport.TrySetResult();
    }

    void Capture()
    {
        bool opened = false, released = true, complete = false;
        string? error = null;
        WasapiCapture? capture = null;
        var driverStopped = new ManualResetEventSlim();
        Exception? driverError = null;
        try
        {
            using var host = new MMDeviceEnumerator();
            var flow = source == "playback" ? DataFlow.Render : DataFlow.Capture;
            using var endpoint = deviceId is null ? host.GetDefaultAudioEndpoint(flow, Role.Multimedia) : host.GetDevice(deviceId);
            if (endpoint.State != DeviceState.Active || endpoint.DataFlow != flow) throw new InvalidOperationException();
            if (cancel.IsCancellationRequested) return;
            capture = source == "playback" ? new WasapiLoopbackCapture(endpoint) : new WasapiCapture(endpoint);
            var format = capture.WaveFormat;
            var floating = format.Encoding == WaveFormatEncoding.IeeeFloat && format.BitsPerSample == 32;
            if ((!floating && !(format.Encoding == WaveFormatEncoding.Pcm && format.BitsPerSample is 16 or 24 or 32)) ||
                format.Channels is < 1 or > 32 || format.SampleRate is < 8000 or > 192000)
                throw new NotSupportedException();
            sampleRate = format.SampleRate;
            samples = new float[checked(sampleRate * SampleSeconds)];
            capture.RecordingStopped += (_, args) => { driverError = args.Exception; driverStopped.Set(); };
            capture.DataAvailable += (_, args) => ReadAudio(args.Buffer.AsSpan(0, args.BytesRecorded),
                format.Channels, format.BitsPerSample / 8, floating);
            if (cancel.IsCancellationRequested) return;
            // The API may start its native worker before this thread observes
            // Capturing. Start timing at invocation to guarantee a conservative cap.
            nativeStartInvoked = Stopwatch.GetTimestamp();
            capture.StartRecording();
            opened = true;
            using var stop = cancel.Token.Register(capture.StopRecording);
            var startLimit = Stopwatch.StartNew();
            while (capture.CaptureState != CaptureState.Capturing &&
                !driverStopped.IsSet && !cancel.IsCancellationRequested && startLimit.ElapsedMilliseconds < CheckTiming.StartupMs)
                Thread.Sleep(5);
            if (cancel.IsCancellationRequested) return;
            if (capture.CaptureState != CaptureState.Capturing || driverStopped.IsSet)
            {
                error = driverError is null ? "timeout" : DeviceError(driverError, false);
                return;
            }
            firstCapture = Stopwatch.GetTimestamp();
            live.TrySetResult();
            int remaining = CheckTiming.RemainingWindow(nativeStartInvoked);
            int wait = CheckTiming.WaitWindow(cancel.Token.WaitHandle, driverStopped.WaitHandle, remaining);
            complete = wait == WaitHandle.WaitTimeout;
            if (wait == 1 && !cancel.IsCancellationRequested)
                error = driverError is null ? "device_lost" : DeviceError(driverError, true);
        }
        catch (Exception e) { if (!cancel.IsCancellationRequested) error = DeviceError(e, opened); }
        finally
        {
            lock (sampleLaunchLock) Volatile.Write(ref captureClosing, 1);
            cancelSample.Cancel();
            if (capture is not null)
            {
                try
                {
                    capture.StopRecording();
                    if (opened && !driverStopped.Wait(CheckTiming.ReleaseMs)) released = false;
                    else capture.Dispose();
                }
                catch (Exception) { released = false; }
            }
            if (!released) Environment.Exit(3);
            // Dispose precedes this clear, independent of stdout progress.
            // An already transferred buffer belongs to Sample and is covered by Guard.
            DropSamples();
            done.TrySetResult(new(released, complete && !cancel.IsCancellationRequested, error));
        }
    }

    private void ReadAudio(ReadOnlySpan<byte> input, int channels, int width, bool floating)
    {
        if (!CanReport) return;
        int frames = input.Length / (width * channels);
        if (frames <= 0 || frames > sampleRate) return;
        // This lock is only for a fixed memory copy or sample ownership, never
        // inference, device calls, UI or protocol output.
        double energy = 0;
        lock (samplesLock)
        {
            for (int i = 0, pos = 0; i < frames; i++)
            {
                double mono = 0;
                for (int c = 0; c < channels; c++, pos += width)
                {
                    var bytes = input.Slice(pos, width);
                    float n = floating ? BitConverter.Int32BitsToSingle(BinaryPrimitives.ReadInt32LittleEndian(bytes)) :
                        width switch {
                            2 => BinaryPrimitives.ReadInt16LittleEndian(bytes) / 32768f,
                            3 => (bytes[0] | bytes[1] << 8 | (sbyte)bytes[2] << 16) / 8388608f,
                            _ => BinaryPrimitives.ReadInt32LittleEndian(bytes) / 2147483648f
                        };
                    if (float.IsFinite(n))
                    {
                        double finite = Math.Clamp(n, -1f, 1f);
                        energy += finite * finite;
                        mono += finite;
                    }
                }
                if (acceptSamples && samples is not null && collected < samples.Length)
                    samples[collected++] = (float)(mono / channels);
            }
        }
        int value = CheckTiming.Meter(energy, frames, channels);
        Interlocked.Exchange(ref latestPacket, Stopwatch.GetTimestamp());
        Volatile.Write(ref level, value);
        if (value >= 17) Interlocked.Exchange(ref signal, 1);
    }

    float[]? TakeSamples()
    {
        lock (samplesLock)
        {
            float[]? data = samples;
            samples = null;
            acceptSamples = false;
            if (data is null) return null;
            if (collected == 0) { Array.Clear(data); return null; }
            // Unfilled tail is zero; enough for the optional short decoder to flush.
            return data;
        }
    }
    void DropSamples()
    {
        lock (samplesLock)
        {
            acceptSamples = false;
            if (samples is not null) Array.Clear(samples);
            samples = null;
        }
    }

    async Task Sample()
    {
        float[]? audio = null;
        bool reportedModel = false;
        try
        {
            // A single optional, bounded scratch path; never referenced by
            // capture deadlines. No provider or Vietnamese task is available here.
            if (!CanReport) return;
            using var modelLease = Session.LoadModel(packageRoot);
            var model = modelLease.Recognizer;
            if (!CanReport) return;
            Program.CheckModel(this, true);
            reportedModel = true;
            while (Stopwatch.GetElapsedTime(firstCapture).TotalMilliseconds < SampleAfterMs && CanReport)
                await Task.Delay(50, cancelSample.Token);
            if (!CanReport || Volatile.Read(ref signal) == 0) return;
            audio = TakeSamples();
            if (audio is null) return;
            using var stream = model.CreateStream();
            stream.AcceptWaveform(sampleRate, audio);
            stream.AcceptWaveform(sampleRate, new float[sampleRate]);
            stream.InputFinished();
            while (CanReport && model.IsReady(stream)) model.Decode(stream);
            if (CanReport) Program.CheckSample(this, model.GetResult(stream).Text);
        }
        catch (OperationCanceledException) when (cancelSample.IsCancellationRequested) { }
        catch (Exception) { if (!reportedModel && CanReport) Program.CheckModel(this, false); }
        finally
        {
            if (audio is not null) Array.Clear(audio);
            DropSamples();
        }
    }

    static string DeviceError(Exception error, bool opened) => error switch
    {
        UnauthorizedAccessException => "permission_denied",
        COMException e when unchecked((uint)e.HResult) == 0x80070005 => "permission_denied",
        _ => opened ? "device_lost" : "device_missing",
    };
}
