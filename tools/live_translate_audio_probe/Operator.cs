using System.Diagnostics;
using System.Text;
using NAudio.CoreAudioApi;

static partial class AudioProbe
{
    static int OperatorMode(MMDeviceEnumerator host)
    {
        // Device names are only presented to the local operator; never support a redirected
        // operator session. The SHARE record below is an allowlist and contains neither IDs nor names.
        if (Console.IsOutputRedirected || Console.IsInputRedirected)
        {
            Console.Error.WriteLine("Operator mode requires an interactive terminal.");
            return 2;
        }
        Console.OutputEncoding = new UTF8Encoding(false);
        Console.WriteLine("Kiểm tra tai nghe và mic • Mỗi lượt đo 5 giây, rồi tự mở lại đúng nguồn cũ 1 giây để kiểm tra nhả nguồn.");
        Console.WriteLine("Chỉ đọc tạm tín hiệu từ thiết bị mày chọn trong bộ nhớ. Tiếng máy gồm mọi âm thanh phát qua thiết bị đó.");
        Console.WriteLine("Không tạo bản ghi, không lưu hay truyền âm thanh. Tắt cuộc họp, thông báo và các app phát tiếng.");
        Console.WriteLine("Nhập mã thử do milo gửi (6–16 ký tự A–Z, 0–9):");
        string? token = Console.ReadLine()?.Trim();
        if (token is null || token.Length is < 6 or > 16 ||
            token.Any(c => c is not (>= 'A' and <= 'Z' or >= '0' and <= '9')))
            return 2;
        var selectedOut = SelectEndpoint(host, DataFlow.Render, "tai nghe vật lý");
        var selectedMic = SelectEndpoint(host, DataFlow.Capture, "mic vật lý");
        if (selectedOut is null || selectedMic is null) return 2;
        if (!Confirm("Xác nhận đã chọn tai nghe và mic vật lý riêng biệt, không dùng thiết bị ảo."))
            return 2;

        var timer = Stopwatch.StartNew();
        var evidence = new List<object>();
        if (!OperatorPair(host, "playback", selectedOut, "headphones_signal", timer, evidence,
            "Tắt/mute mic. Phát một âm thử qua đúng tai nghe, giữ 6 giây hoặc lâu hơn.")) return 2;
        if (!OperatorPair(host, "mic", selectedMic, "mic_quiet_with_playback", timer, evidence,
            "Giữ âm thử phát qua tai nghe; mic vẫn mute. Không nói.")) return 2;
        if (!OperatorPair(host, "mic", selectedMic, "mic_signal", timer, evidence,
            "Dừng âm thử để tai nghe im lặng. Bật lại mic. Sau khi bắt đầu, đếm 1 đến 6 vào mic.")) return 2;
        if (!OperatorPair(host, "playback", selectedOut, "playback_quiet_with_mic", timer, evidence,
            "Giữ tai nghe im lặng. Sau khi bắt đầu, đếm 1 đến 6 vào mic.")) return 2;

        if (!Confirm("Xong, hiện một dòng tổng hợp không có tên hay mã thiết bị để mày tự gửi?"))
            return 0;
        Console.Write("SHARE ");
        Output(new { schema = "sig4a-audio-proof-v1", token, physical_operator_confirmed = true,
            samples_persisted = false, observations = evidence });
        return 0;
    }

    static string? SelectEndpoint(MMDeviceEnumerator host, DataFlow flow, string label)
    {
        var ids = new List<string>();
        Console.WriteLine($"Chọn {label}, hoặc Enter để thoát:");
        foreach (var device in host.EnumerateAudioEndPoints(flow, DeviceState.Active))
        {
            using (device)
            {
                ids.Add(device.ID);
                Console.WriteLine($"  {ids.Count}. {device.FriendlyName}");
            }
        }
        return int.TryParse(Console.ReadLine(), out int number) && number >= 1 && number <= ids.Count
            ? ids[number - 1] : null;
    }

    static bool Confirm(string message)
    {
        Console.WriteLine(message);
        Console.WriteLine("Gõ OK rồi Enter để tiếp tục, hoặc chỉ Enter để thoát.");
        return Console.ReadLine()?.Trim() == "OK";
    }

    static bool OperatorPair(MMDeviceEnumerator host, string source, string id, string condition,
        Stopwatch clock, List<object> evidence, string instruction)
    {
        if (!Confirm(instruction + " Đồng ý đo 5 giây rồi tự mở lại đúng nguồn này 1 giây?"))
            return false;
        Console.WriteLine("Đang đo. Ctrl+C để dừng công cụ.");
        // If the audio driver stalls even inside Dispose, kill this isolated, audio-only process.
        // Nothing can restart the session or launch another capture automatically after a timeout.
        using var watchdog = new Timer(_ =>
        {
            Console.Error.WriteLine("Thiết bị không phản hồi. Đã dừng, không tự nghe lại.");
            Environment.Exit(124);
        }, null, TimeSpan.FromSeconds(18), Timeout.InfiniteTimeSpan);
        var first = Measure(host, source, id, 5, "initial", clock);
        var second = Measure(host, source, id, 1, "reopen", clock);
        watchdog.Change(Timeout.InfiniteTimeSpan, Timeout.InfiniteTimeSpan);
        foreach (var signal in new[] { first, second })
            evidence.Add(new { condition, signal.source, signal.stage, signal.samples_measured,
                signal.rms_dbfs, signal.peak_dbfs, signal.signal_observed,
                signal.started_ms, signal.stop_requested_ms, signal.disposed_ms });
        return true;
    }
}
