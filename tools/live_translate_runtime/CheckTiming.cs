static class CheckTiming
{
    internal const int StartupMs = 3_000, WindowMs = 5_000, ReleaseMs = 1_400;
    internal const int MeterMs = 200, SampleReleaseMs = 220;

    internal static int RemainingWindow(long startTimestamp) =>
        Math.Max(0, (int)Math.Floor(WindowMs - System.Diagnostics.Stopwatch.GetElapsedTime(startTimestamp).TotalMilliseconds));

    internal static bool CanReuseAfterSample(Task? sample) =>
        sample is null || Task.WaitAny([sample], SampleReleaseMs) != -1;

    // Called from the independent guard before any successful release ACK. A
    // cancelled native decoder that remains in LoadModel/Decode owns this helper.
    internal static void RetireSampleOrExit(Task? sample)
    {
        if (!CanReuseAfterSample(sample)) Environment.Exit(3);
    }

    // Called on the native capture-owner thread, never from the UI or ASR thread.
    internal static int WaitWindow(WaitHandle cancelled, WaitHandle driverStopped, int duration = WindowMs) =>
        WaitHandle.WaitAny([cancelled, driverStopped], duration);

    internal static int Meter(double energy, int frames, int channels)
    {
        if (!double.IsFinite(energy) || energy <= 0 || frames <= 0 || channels <= 0) return 0;
        double rms = Math.Sqrt(energy / ((double)frames * channels));
        double db = 20 * Math.Log10(rms);
        return (int)Math.Round(Math.Clamp((db + 60) * 100 / 60, 0, 100));
    }
}
