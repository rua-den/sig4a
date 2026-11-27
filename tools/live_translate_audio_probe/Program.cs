using System.Buffers.Binary;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text.Json;
using NAudio.CoreAudioApi;
using NAudio.Wave;

static class AudioProbe
{
    static void Output(object value) => Console.WriteLine(JsonSerializer.Serialize(value));

    [MTAThread]
    static int Main(string[] args)
    {
        if (!OperatingSystem.IsWindows()) return 2;
        try
        {
            using var host = new MMDeviceEnumerator();
            if (args is ["--list"])
            {
                var endpoints = new List<object>();
                foreach (var flow in new[] { DataFlow.Render, DataFlow.Capture })
                foreach (var endpoint in host.EnumerateAudioEndPoints(flow, DeviceState.Active))
                {
                    using (endpoint)
                        endpoints.Add(new { source = flow == DataFlow.Render ? "playback" : "mic",
                            id = endpoint.ID, name = endpoint.FriendlyName });
                }
                Output(new { ok = true, endpoints });
                return 0;
            }

            if (args.Length < 3 || args.Length > 5 || args.Length % 2 == 0 ||
                !int.TryParse(args[0], out int seconds) || seconds is < 1 or > 15)
                throw new ArgumentException();
            // Validate both selections without opening either audio stream.
            var sources = new List<(string source, string id)>();
            for (int i = 1; i < args.Length; i += 2)
            {
                var source = args[i];
                var flow = source switch { "mic" => DataFlow.Capture, "playback" => DataFlow.Render,
                    _ => throw new ArgumentException() };
                if (source == "mic" && args[i+1] == "default") throw new ArgumentException();
                using var device = args[i+1] == "default"
                    ? host.GetDefaultAudioEndpoint(flow, Role.Multimedia)
                    : host.GetDevice(args[i+1]);
                if (device.DataFlow != flow || device.State != DeviceState.Active)
                    throw new ArgumentException();
                sources.Add((source, device.ID));
            }

            var timer = Stopwatch.StartNew();
            foreach (var (source, id) in sources)
            {
                Measure(host, source, id, seconds, "initial", timer);
                Measure(host, source, id, 1, "reopen", timer);
            }
            Output(new { ok = true, persisted_audio = false, all_streams_disposed = true });
            return 0;
        }
        catch (Exception error)
        {
            Output(new { ok = false, error = error.GetType().Name,
                hresult = $"0x{error.HResult:X8}", persisted_audio = false });
            return 1;
        }
    }

    static void Measure(MMDeviceEnumerator host, string source, string id, int seconds,
        string stage, Stopwatch timer)
    {
        string name;
        long count = 0, bytes = 0;
        double sums = 0, peak = 0;
        long started, stopRequested;
        using (var endpoint = host.GetDevice(id))
        using (WasapiCapture capture = source == "playback"
                ? new WasapiLoopbackCapture(endpoint) : new WasapiCapture(endpoint))
        {
            name = endpoint.FriendlyName;
            var format = capture.WaveFormat;
            var floating = format.Encoding == WaveFormatEncoding.IeeeFloat && format.BitsPerSample == 32;
            var pcm = format.Encoding == WaveFormatEncoding.Pcm && format.BitsPerSample is 16 or 24 or 32;
            if (!floating && !pcm) throw new NotSupportedException();
            int stride = format.BitsPerSample / 8;
            var stopped = new TaskCompletionSource<Exception?>(TaskCreationOptions.RunContinuationsAsynchronously);
            capture.RecordingStopped += (_, e) => stopped.TrySetResult(e.Exception);
            capture.DataAvailable += (_, e) =>
            {
                // Samples are only read directly from WASAPI's transient buffer.
                // Store aggregates only; no file output or transfer of PCM.
                bytes += e.BytesRecorded;
                for (int i = 0; i + stride <= e.BytesRecorded; i += stride)
                {
                    var slice = e.Buffer.AsSpan(i, stride);
                    double sample = floating
                        ? BitConverter.Int32BitsToSingle(BinaryPrimitives.ReadInt32LittleEndian(slice))
                        : stride switch
                        {
                            2 => BinaryPrimitives.ReadInt16LittleEndian(slice) / 32768d,
                            3 => (slice[0] | slice[1] << 8 | ((sbyte)slice[2] << 16)) / 8388608d,
                            _ => BinaryPrimitives.ReadInt32LittleEndian(slice) / 2147483648d
                        };
                    if (!double.IsFinite(sample)) continue;
                    sums += sample * sample;
                    peak = Math.Max(peak, Math.Abs(sample));
                    count++;
                }
            };
            started = timer.ElapsedMilliseconds;
            capture.StartRecording();
            // Loopback can legitimately deliver no nonempty packets when nothing renders.
            if (stopped.Task.Wait(TimeSpan.FromSeconds(seconds)))
                throw new InvalidOperationException("Capture ended early.", stopped.Task.Result);
            stopRequested = timer.ElapsedMilliseconds;
            capture.StopRecording();
            if (!stopped.Task.Wait(TimeSpan.FromSeconds(3))) throw new TimeoutException();
            if (stopped.Task.Result is { } error)
                throw new InvalidOperationException("Audio stopped with an error.", error);
        }
        Output(new { ok = true, source, endpoint_id = id, endpoint_name = name, stage,
            requested_seconds = seconds, bytes_measured = bytes, samples_measured = count,
            rms_dbfs = sums > 0 ? (double?)Math.Round(10 * Math.Log10(sums / count), 2) : null,
            peak_dbfs = peak > 0 ? (double?)Math.Round(20 * Math.Log10(peak), 2) : null,
            signal_observed = peak > 0.001,
            started_ms = started, stop_requested_ms = stopRequested,
            disposed_ms = timer.ElapsedMilliseconds, persisted_audio = false });
    }
}
