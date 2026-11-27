using System.Buffers.Binary;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Threading.Channels;
using NAudio.CoreAudioApi;
using NAudio.Wave;
using SherpaOnnx;

sealed partial class Session(long id, string source, string? deviceId, bool translate, WindowsModelPaths.Pin packageRoot)
{
    internal long Id => id;
    OfflineTranslation? translator;
    Task? translationWork;
    long latestSequence;
    internal long LatestSequence => Interlocked.Read(ref latestSequence);
    internal Task Work { get; set; } = Task.CompletedTask;
    readonly CancellationTokenSource cancel = new();
    internal bool Cancelled => cancel.IsCancellationRequested;
    readonly Channel<float[]> audio = Channel.CreateBounded<float[]>(new BoundedChannelOptions(16)
    {
        SingleReader = true, SingleWriter = true, FullMode = BoundedChannelFullMode.Wait
    });
    internal void Stop() => cancel.Cancel();

    internal sealed class RecognizerLease(OnlineRecognizer recognizer, EnglishModelInstaller.AssetLease assets) : IDisposable
    {
        internal OnlineRecognizer Recognizer { get; } = recognizer;
        public void Dispose()
        {
            try { Recognizer.Dispose(); }
            finally { assets.Dispose(); }
        }
    }

    internal static RecognizerLease LoadModel(WindowsModelPaths.Pin packageRoot)
    {
        var names = EnglishModelPins.ModelFiles;
        var assets = EnglishModelInstaller.OpenInstalled(packageRoot, EnglishModelPins.Pins, CancellationToken.None);
        try
        {
            var config = new OnlineRecognizerConfig();
            config.FeatConfig.SampleRate = 16000;
            config.FeatConfig.FeatureDim = 80;
            config.ModelConfig.Tokens = assets.NativeName(names[0]);
            config.ModelConfig.Transducer.Encoder = assets.NativeName(names[1]);
            config.ModelConfig.Transducer.Decoder = assets.NativeName(names[2]);
            config.ModelConfig.Transducer.Joiner = assets.NativeName(names[3]);
            config.ModelConfig.Provider = "cpu";
            config.ModelConfig.NumThreads = Math.Clamp(Environment.ProcessorCount, 1, 4);
            config.DecodingMethod = "greedy_search";
            config.EnableEndpoint = 1;
            config.Rule1MinTrailingSilence = 1.8f;
            config.Rule2MinTrailingSilence = .60f;
            config.Rule3MinUtteranceLength = 18f;
            return new RecognizerLease(new OnlineRecognizer(config), assets);
        }
        catch { assets.Dispose(); throw; }
    }

    internal async Task Run()
    {
        var stopped = new TaskCompletionSource<Exception?>(TaskCreationOptions.RunContinuationsAsynchronously);
        WasapiCapture? capture = null;
        string? failure = null;
        bool issuedStart = false;
        bool released = true;
        try
        {
            using var modelLease = LoadModel(packageRoot); // Pin file/namespace guards for native recognizer lifetime.
            var model = modelLease.Recognizer;
            if (Cancelled) return;
            using var stream = model.CreateStream();
            using var host = new MMDeviceEnumerator();
            var flow = source == "playback" ? DataFlow.Render : DataFlow.Capture;
            using var endpoint = deviceId is null
                ? host.GetDefaultAudioEndpoint(flow, Role.Multimedia) : host.GetDevice(deviceId);
            if (endpoint.State != DeviceState.Active || endpoint.DataFlow != flow)
            {
                failure = "device_missing";
                return;
            }
            if (Cancelled) return;
            capture = source == "playback" ? new WasapiLoopbackCapture(endpoint) : new WasapiCapture(endpoint);
            var format = capture.WaveFormat;
            var floats = format.Encoding == WaveFormatEncoding.IeeeFloat && format.BitsPerSample == 32;
            if (!floats && !(format.Encoding == WaveFormatEncoding.Pcm && format.BitsPerSample is 16 or 24 or 32) ||
                format.Channels is < 1 or > 32 || format.SampleRate is < 8000 or > 192000)
            {
                failure = "device_missing";
                return;
            }
            int width = format.BitsPerSample / 8;
            capture.RecordingStopped += (_, e) => { stopped.TrySetResult(e.Exception); audio.Writer.TryComplete(); };
            capture.DataAvailable += (_, e) =>
            {
                if (Cancelled || e.BytesRecorded == 0) return;
                int frames = e.BytesRecorded / (width * format.Channels);
                if (frames <= 0 || frames > format.SampleRate)
                {
                    audio.Writer.TryComplete(new InvalidDataException());
                    return;
                }
                var mono = new float[frames];
                for (int i = 0, j = 0; i < frames; i++)
                {
                    double sum = 0;
                    for (int ch = 0; ch < format.Channels; ch++, j += width)
                    {
                        var data = e.Buffer.AsSpan(j, width);
                        float x = floats ? BitConverter.Int32BitsToSingle(BinaryPrimitives.ReadInt32LittleEndian(data))
                            : width switch
                            {
                                2 => BinaryPrimitives.ReadInt16LittleEndian(data) / 32768f,
                                3 => (data[0] | data[1] << 8 | ((sbyte)data[2] << 16)) / 8388608f,
                                _ => BinaryPrimitives.ReadInt32LittleEndian(data) / 2147483648f
                            };
                        if (float.IsFinite(x)) sum += Math.Clamp(x, -1f, 1f);
                    }
                    mono[i] = (float)(sum / format.Channels);
                }
                // An overrun terminates rather than silently skipping or accumulating audio.
                if (!audio.Writer.TryWrite(mono)) audio.Writer.TryComplete(new InvalidDataException());
            };
            if (Cancelled) return;
            capture.StartRecording();
            issuedStart = true;
            // Detach the input promptly even if recognition is currently executing.
            // Registration invokes immediately if Stop already won the race.
            using var stopAudio = cancel.Token.Register(capture.StopRecording);
            // Wait for actual WASAPI Start on NAudio's capture thread, not just its scheduling.
            var startup = Stopwatch.StartNew();
            while (capture.CaptureState != CaptureState.Capturing && !stopped.Task.IsCompleted &&
                startup.ElapsedMilliseconds < 2_000)
                await Task.Delay(10, cancel.Token).ConfigureAwait(false);
            if (capture.CaptureState != CaptureState.Capturing || stopped.Task.IsCompleted)
            {
                failure = "device_missing";
                if (stopped.Task.IsCompletedSuccessfully && stopped.Task.Result is { } cause)
                    failure = DeviceCode(cause, issuedStart: false);
                return;
            }
            if (!Program.Started(this)) return;
            if (translate)
            {
                translator = new OfflineTranslation(
                    (seq, en, vi) => Program.Translated(this, seq, en, vi),
                    () => Program.TranslatorUnavailable(this), cancel.Token);
                translationWork = Task.Run(translator.Run);
            }
            var clock = Stopwatch.StartNew();
            long lastData = 0, paddedTo = 0;
            var silence = new float[format.SampleRate / 20];
            while (!Cancelled)
            {
                if (audio.Reader.TryRead(out var mono))
                {
                    lastData = paddedTo = clock.ElapsedMilliseconds;
                    Advance(mono);
                }
                else if (audio.Reader.Completion.IsCompleted)
                {
                    failure = stopped.Task.IsCompleted ? "device_lost" : "internal";
                    break;
                }
                else
                {
                    // WASAPI loopback may deliver no packets once render becomes idle. Fill only
                    // sustained gaps so a final phrase can endpoint without a new sound.
                    if (source == "playback" && clock.ElapsedMilliseconds - lastData >= 250 &&
                        clock.ElapsedMilliseconds - paddedTo >= 50)
                    {
                        paddedTo = Math.Max(clock.ElapsedMilliseconds - 250, paddedTo) + 50;
                        Advance(silence);
                    }
                    await Task.Delay(25, cancel.Token).ConfigureAwait(false);
                }
            }

            void Advance(float[] pcm)
            {
                // Sherpa handles real hardware sample rates using its streaming resampler.
                stream.AcceptWaveform(format.SampleRate, pcm);
                while (!Cancelled && model.IsReady(stream)) model.Decode(stream);
                if (Cancelled || !model.IsEndpoint(stream)) return;
                var sentence = model.GetResult(stream).Text;
                model.Reset(stream);
                sentence = Program.TextCap(sentence.Trim(), 2048);
                if (sentence.Length == 0) return;
                var seq = Interlocked.Increment(ref latestSequence);
                Program.Caption(this, sentence, seq);
                translator?.Enqueue(seq, sentence);
            }
        }
        catch (OperationCanceledException) when (Cancelled) { }
        catch (Exception e) { failure = DeviceCode(e, issuedStart); }
        finally
        {
            // Stop inference and source concurrently. Capture and child reaping
            // share a deadline so no unbounded ACK precedes the parent's kill.
            var disposeClock = Stopwatch.StartNew();
            bool stopping = Cancelled;
            cancel.Cancel();
            audio.Writer.TryComplete();
            if (capture is not null)
            {
                try
                {
                    capture.StopRecording();
                    if (issuedStart && await Task.WhenAny(stopped.Task, Task.Delay(1_500)) != stopped.Task)
                        released = false;
                    else capture.Dispose();
                }
                catch (Exception) { released = false; }
            }
            while (audio.Reader.TryRead(out var discarded)) Array.Clear(discarded);
            if (translationWork is not null)
            {
                int remaining = Math.Max(0, 1_800 - (int)disposeClock.ElapsedMilliseconds);
                if (await Task.WhenAny(translationWork, Task.Delay(remaining)) != translationWork ||
                    translator?.Released != true) released = false;
            }
            Program.Finished(this, failure, released, stopping);
        }
    }

    static string DeviceCode(Exception error, bool issuedStart) => error switch
    {
        FileNotFoundException or DirectoryNotFoundException or SetupFailure => "model_missing",
        DllNotFoundException or EntryPointNotFoundException or BadImageFormatException => "backend_missing",
        UnauthorizedAccessException => "permission_denied",
        COMException c when unchecked((uint)c.HResult) == 0x80070005 => "permission_denied",
        COMException => issuedStart ? "device_lost" : "device_missing",
        _ => "internal",
    };
}
