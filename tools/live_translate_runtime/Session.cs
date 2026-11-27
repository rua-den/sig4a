using System.Buffers.Binary;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Threading.Channels;
using NAudio.CoreAudioApi;
using NAudio.Wave;
using SherpaOnnx;

sealed partial class Session(long id, string source, string? deviceId, bool translate)
{
    internal long Id => id;
    internal bool WantsTranslation => translate;
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

    static readonly string ModelDir = Path.Combine(AppContext.BaseDirectory, "models", "zipformer");
    const string ModelStem = "epoch-99-avg-1-chunk-16-left-128";
    static OnlineRecognizer LoadModel()
    {
        var names = new[] { "tokens.txt", $"encoder-{ModelStem}.int8.onnx", $"decoder-{ModelStem}.onnx", $"joiner-{ModelStem}.int8.onnx" };
        if ((File.GetAttributes(Path.GetDirectoryName(ModelDir)!) & FileAttributes.ReparsePoint) != 0 ||
            (File.GetAttributes(ModelDir) & FileAttributes.ReparsePoint) != 0) throw new FileNotFoundException();
        var expected = new[] {
            "49e3c2646595fd907228b3c6787069658f67b17377c60aeb8619c4551b2316fb",
            "5022b2eca5b19d1bc104fcf33e26bc32604b7df553cd2e1f62e31dc7b05e9c87",
            "85914f840b71110487ece7a30ebd0bd4b8fb5c2623215bba3aa5ab4c138fc591",
            "abd5e30f3f16fc510605c6029dba33f10e4386bd75c5bdc30cf94076864db10d",
        };
        for (int i = 0; i < names.Length; i++)
        {
            var f = new FileInfo(Path.Combine(ModelDir, names[i]));
            if (!f.Exists || f.Length is < 100 or > 250_000_000 || (f.Attributes & FileAttributes.ReparsePoint) != 0)
                throw new FileNotFoundException();
            using var data = f.OpenRead();
            if (!Convert.ToHexString(SHA256.HashData(data)).Equals(expected[i], StringComparison.OrdinalIgnoreCase))
                throw new FileNotFoundException();
        }
        var config = new OnlineRecognizerConfig();
        config.FeatConfig.SampleRate = 16000;
        config.FeatConfig.FeatureDim = 80;
        config.ModelConfig.Tokens = Path.Combine(ModelDir, names[0]);
        config.ModelConfig.Transducer.Encoder = Path.Combine(ModelDir, names[1]);
        config.ModelConfig.Transducer.Decoder = Path.Combine(ModelDir, names[2]);
        config.ModelConfig.Transducer.Joiner = Path.Combine(ModelDir, names[3]);
        config.ModelConfig.Provider = "cpu";
        config.ModelConfig.NumThreads = Math.Clamp(Environment.ProcessorCount, 1, 4);
        config.DecodingMethod = "greedy_search";
        config.EnableEndpoint = 1;
        config.Rule1MinTrailingSilence = 1.8f;
        config.Rule2MinTrailingSilence = .60f;
        config.Rule3MinUtteranceLength = 18f;
        return new OnlineRecognizer(config);
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
            using var model = LoadModel(); // Fail before accessing the endpoint if unavailable.
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
                translator = new OfflineTranslation(this, cancel.Token);
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
            // Translation cannot survive Stop, device loss, quit or a source change.
            if (translationWork is not null)
            {
                // Program.Finished distinguishes user cancellation from internal failure.
                bool stopping = Cancelled;
                cancel.Cancel();
                if (await Task.WhenAny(translationWork, Task.Delay(2_750)) != translationWork ||
                    translator?.Released != true) released = false;
                Program.Finished(this, failure, released, stopping);
            }
            else Program.Finished(this, failure, released, Cancelled);
        }
    }

    static string DeviceCode(Exception error, bool issuedStart) => error switch
    {
        FileNotFoundException or DirectoryNotFoundException => "model_missing",
        DllNotFoundException or EntryPointNotFoundException or BadImageFormatException => "backend_missing",
        UnauthorizedAccessException => "permission_denied",
        COMException c when unchecked((uint)c.HResult) == 0x80070005 => "permission_denied",
        COMException => issuedStart ? "device_lost" : "device_missing",
        _ => "internal",
    };
}
