param([Parameter(Mandatory)][string]$Directory)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech
New-Item -ItemType Directory -Force $Directory | Out-Null
$fixture = Get-Content -LiteralPath "$PSScriptRoot\fixture-manifest.json" -Raw -Encoding UTF8 | ConvertFrom-Json
$tts = New-Object System.Speech.Synthesis.SpeechSynthesizer
try {
    $voices = @($tts.GetInstalledVoices() | Where-Object { $_.Enabled -and $_.VoiceInfo.Culture.Name -like 'en-*' } | Sort-Object {$_.VoiceInfo.Name})
    if ($voices.Count -eq 0) { throw 'Install an English Windows SAPI voice first.' }
    $format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
    $entries = @($fixture.entries | Where-Object kind -eq utterance) + @([pscustomobject]@{id='WARM'; text='This sentence warms the models and is never scored.'})
    $voiceInfo = [ordered]@{}
    for ($i=0; $i -lt $entries.Count; $i++) {
        $row = $entries[$i]
        $voice = $voices[$i % $voices.Count].VoiceInfo
        $tts.SelectVoice($voice.Name)
        $rate = @(0, 1, -1, 2, -2, 0, 1)[$i % 7]
        $tts.Rate = $rate
        $tts.SetOutputToWaveFile((Join-Path $Directory ($row.id + '.wav')), $format)
        try { $tts.Speak($row.text) } finally { $tts.SetOutputToNull() }
        $voiceInfo[$row.id] = @{name=$voice.Name; culture=$voice.Culture.Name; rate=$rate}
    }
    $voiceInfo | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $Directory 'sapi-voices.json') -Encoding UTF8
} finally { $tts.Dispose() }
