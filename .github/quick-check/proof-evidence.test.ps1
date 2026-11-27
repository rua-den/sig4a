$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'proof-evidence.ps1')
$match = Convert-SignedQuickFixtureStatus 'Translation' "Translation · Mẫu khớp. Ollama báo Cloud đã tắt. · 0.42 giây"
if ($match.result -cne 'match' -or $match.elapsed_ms -ne 420) { throw 'Exact match status/timing lost.' }
$fail = Convert-SignedQuickFixtureStatus 'Grammar' "Grammar · Chưa xác minh được tiến trình Ollama chính thức trên cổng mặc định. · 0.08 giây"
if ($fail.result -cne 'unverified_owner' -or $fail.elapsed_ms -ne 80) { throw 'Owner rejection should be evidence, not a missing result.' }
$mismatch = Convert-SignedQuickFixtureStatus 'Translation' "Translation  · Có phản hồi, mẫu chưa khớp; xem kết quả. · 11.20 giây"
if ($mismatch.result -cne 'mismatch' -or $mismatch.elapsed_ms -ne 11200) { throw 'Mismatch incorrectly counted as match.' }
if (Convert-SignedQuickFixtureStatus 'Grammar' 'Translation · Mẫu khớp. Ollama báo Cloud đã tắt.') { throw 'Copied verdict across fixtures.' }
if (Convert-SignedQuickFixtureStatus 'Grammar' 'Grammar · arbitrary generated text') { throw 'Unlisted UI text must not emit evidence.' }
$example = '[GIN] 2026/11/27 - 21:12:17 | 200 |       17.682ms |       127.0.0.1 | POST     "/api/chat"'
$r = Convert-IsolatedSignedRequestLine $example
if ($r.route -cne '/api/chat' -or $r.method -cne 'POST' -or $r.status -ne 200 -or $r.duration -cne '17.682ms') { throw 'Fixed verified log request not decoded.' }
foreach ($reject in @($example.Replace('127.0.0.1','192.168.1.4'),$example.Replace('POST','GET'),$example.Replace('/api/chat','/api/chat?content=secret'),"warning: raw message")) {
    if (Convert-IsolatedSignedRequestLine $reject) { throw 'Unexpected or private log line survived sanitization.' }
}
$scratch = Join-Path ([IO.Path]::GetTempPath()) ('qc-evidence-test-'+[guid]::NewGuid())
try {
    New-Item -ItemType Directory -Path $scratch | Out-Null
    Set-Content -Encoding UTF8NoBOM -LiteralPath (Join-Path $scratch 'server.out') -Value ($example.Replace('POST','GET').Replace('/api/chat','/api/tags'))
    Set-Content -Encoding UTF8NoBOM -LiteralPath (Join-Path $scratch 'server.err') -Value 'old private model trace'
    $before = Get-IsolatedSignedLogOffsets $scratch
    Add-Content -Encoding UTF8NoBOM -LiteralPath (Join-Path $scratch 'server.out') -Value @($example,'private model trace',($example.Replace('17.682ms','30.124s').Replace('200','499')))
    $evidence = Get-IsolatedSignedCompletedRequests $scratch $before
    if (!$evidence.chat_completion_seen -or $evidence.completed.Count -ne 2 -or
        @($evidence.completed | Where-Object {$_.status -eq 499}).Count -ne 1 -or
        ($evidence|ConvertTo-Json -Depth 5) -match 'private model trace|/api/tags') { throw 'Per-click log proof mixed segments or emitted unsafe text.' }
    $next = Get-IsolatedSignedCompletedRequests $scratch (Get-IsolatedSignedLogOffsets $scratch)
    if ($next.chat_completion_seen -or $next.completed.Count -ne 0) { throw 'Missing completion falsely counted as observed.' }
} finally {
    Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
}
'Fixed bilingual terminal evidence, unknown-text refusal, and per-click local route timing checks passed.'
