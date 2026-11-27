# Windows PowerShell 5.1: deterministic synthetic speech, no microphone.
param([Parameter(Mandatory)][string]$Output)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Speech
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
try {
    $form = [System.Speech.AudioFormat.SpeechAudioFormatInfo]::new(
        16000,[System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,[System.Speech.AudioFormat.AudioChannel]::Mono)
    $synth.SelectVoiceByHints([System.Speech.Synthesis.VoiceGender]::NotSet,
        [System.Speech.Synthesis.VoiceAge]::NotSet,0,[cultureinfo]'en-US')
    $synth.SetOutputToWaveFile($Output,$form)
    $synth.Speak('The meeting starts at three. Please open the window.')
} finally { $synth.Dispose() }
