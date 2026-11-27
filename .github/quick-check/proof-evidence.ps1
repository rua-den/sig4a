# CI-only evidence from the isolated ordinary Quick Check / signed fixture.
# No prompt, generated preview, generic daemon line, host path, or response body leaves this helper.

function Get-SignedQuickFixtureStatusPattern([string]$Label) {
    if ($Label -cnotin @('Translation','Grammar')) { throw 'Unknown Quick Check fixture.' }
    $caption = @{
        match = 'Mẫu khớp. Ollama báo Cloud đã tắt.'
        mismatch = 'Có phản hồi, mẫu chưa khớp; xem kết quả.'
        unsupported_selection = 'Mẫu thử chỉ hỗ trợ Ollama chính thức trên cổng mặc định với gemma4:e2b đã có sẵn.'
        unverified_owner = 'Chưa xác minh được tiến trình Ollama chính thức trên cổng mặc định.'
        ollama_unreachable = 'Không kết nối được Ollama ở cổng mặc định.'
        cloud_unverified = 'Ollama chưa báo Cloud tắt bằng biến môi trường. Xem thiết lập Ollama trong Local AI.'
        unsupported_response = 'Phiên bản hoặc phản hồi của Ollama chưa được hỗ trợ.'
        unverified_alias = 'Không xác minh được model local đã chọn.'
        model_missing = 'Chưa có gemma4:e2b trong Ollama. Xem Local AI.'
        timeout = 'Đã hết thời gian chờ. Không tự chạy lại.'
        invalid_response = 'Phản hồi không hợp lệ hoặc chưa hoàn tất.'
        config_unavailable = 'Không đọc được cấu hình đã lưu.'
        configuration_changed = 'Cấu hình đã đổi. Hãy thử lại.'
        busy = 'Local AI đang bận. Hãy thử lại khi công việc hiện tại kết thúc.'
        view_lost = 'Phiên kiểm tra đã đóng. Mở lại mục này để thử.'
        internal = 'Không thể bắt đầu kiểm tra.'
        cancelled = 'Đã huỷ kiểm tra.'
        stale = 'Kết quả không còn hiệu lực. Thử lại nếu cần.'
        bridge_error = 'Không kết nối được với dashboard Windows.'
        unknown_failure = 'Không xác minh được mẫu.'
    }
    # Permit accessibility whitespace folding, but all content and the fixture
    # label must match the app's known strings. Never consume model output.
    $alts = @($caption.GetEnumerator() | ForEach-Object {
        "(?<" + $_.Key + ">" + (([regex]::Escape($_.Value) -split '\\ ') -join '\s+') + ")"
    })
    return ('^\s*' + $Label + '\s*·\s*(?:' + ($alts -join '|') + ')(?:\s*·\s*(?<seconds>[0-9]{1,2}[.][0-9]{2})\s+giây)?\s*$')
}

function Convert-SignedQuickFixtureStatus([string]$Label,[string]$AccessibleName) {
    $pattern = Get-SignedQuickFixtureStatusPattern $Label
    if ($AccessibleName.Length -gt 256) { return $null }
    $regex = [regex]::new($pattern)
    $match = $regex.Match($AccessibleName)
    if (!$match.Success) { return $null }
    $code = @($regex.GetGroupNames() | Where-Object { $_ -ne '0' -and $_ -ne 'seconds' -and $match.Groups[$_].Success })
    if ($code.Count -ne 1) { return $null }
    $seconds = $match.Groups['seconds'].Value
    $ms = if ($seconds) { [int]($seconds.Replace('.','')) * 10 } else { $null }
    return @{result=$code[0]; elapsed_ms=$ms}
}

function Get-IsolatedSignedLogOffsets([string]$Scratch) {
    $offsets = @{}
    foreach ($name in @('server.out','server.err')) {
        $file = Join-Path $Scratch $name
        $offsets[$name] = if (Test-Path -LiteralPath $file -PathType Leaf) { (Get-Item -LiteralPath $file).Length } else { 0L }
    }
    return $offsets
}

function Convert-IsolatedSignedRequestLine([string]$Line) {
    if ($Line.Length -gt 360) { return $null }
    $clean = [regex]::Replace($Line,'\x1b\[[0-9;]{1,12}m','')
    $pattern = '^\[GIN\]\s+[^|\r\n]{1,48}\|\s*([1-5][0-9]{2})\s*\|\s*((?:[0-9]{1,4}m)?[0-9]{1,9}(?:[.][0-9]{1,9})?(?:ns|us|µs|μs|ms|s))\s*\|\s*127[.]0[.]0[.]1\s*\|\s*(GET|POST)\s+"(/api/(?:status|tags|chat))"\s*$'
    $m = [regex]::Match($clean,$pattern)
    if (!$m.Success) { return $null }
    $method = $m.Groups[3].Value
    $route = $m.Groups[4].Value
    if (($route -eq '/api/chat' -and $method -ne 'POST') -or ($route -ne '/api/chat' -and $method -ne 'GET')) { return $null }
    return @{method=$method;route=$route;status=[int]$m.Groups[1].Value;duration=$m.Groups[2].Value}
}

function Get-IsolatedSignedCompletedRequests([string]$Scratch,[hashtable]$Offsets) {
    $routes = [Collections.Generic.List[object]]::new()
    $scan = [Collections.Generic.List[string]]::new()
    foreach ($name in @('server.out','server.err')) {
        $path = Join-Path $Scratch $name
        $stream = $null
        try {
            $stream = [IO.File]::Open($path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::ReadWrite)
            $offset = [long]$Offsets[$name]
            $remaining = $stream.Length - $offset
            if ($remaining -lt 0 -or $remaining -gt 65536) { $scan.Add('truncated'); continue }
            $bytes = [byte[]]::new([int]$remaining)
            $stream.Position = $offset
            $count = 0
            while ($count -lt $bytes.Length) {
                $n = $stream.Read($bytes,$count,$bytes.Length-$count)
                if (!$n){break}
                $count += $n
            }
            $text = [Text.Encoding]::UTF8.GetString($bytes,0,$count)
            $lines = $text -split '\r?\n'
            foreach ($line in ($lines | Select-Object -SkipLast 1)) {
                $row = Convert-IsolatedSignedRequestLine $line
                if ($row -and $routes.Count -lt 12) { $routes.Add($row) }
            }
            $scan.Add('read')
        } catch { $scan.Add('unavailable') }
        finally { if ($stream) { $stream.Dispose() } }
    }
    # A missing completion is not proof that a request was never transmitted.
    return @{completed=@($routes.ToArray()); log_reads=@($scan.ToArray()); chat_completion_seen=@($routes | Where-Object {$_.route -eq '/api/chat'}).Count -gt 0}
}
