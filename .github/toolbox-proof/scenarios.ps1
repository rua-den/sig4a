# Native release scenarios. Test-only helpers must not send bridge messages or inject WebView scripts.
function Shortcut-Status([IntPtr]$h,[string]$name){
 [void](Find-Control $h $name '' '' 'state' 15)
}
function Assert-Saved([string]$expected){
 $j=Get-Content -LiteralPath $cfg -Raw|ConvertFrom-Json
 Assert-Proof ($j.global_shortcuts.toolbox -ceq $expected -and $j.global_shortcuts.grammar -ceq 'Ctrl+Alt+G' -and $j.global_shortcuts.translation -ceq 'Ctrl+Alt+T' -and $j.global_shortcuts.ocr -ceq 'Ctrl+Alt+O' -and $j.old_extra_fixture -ceq 'preserve me') 'Unexpected persisted shortcuts or damage to unrelated config'
}
function Assert-Focus-Input([IntPtr]$h,[string]$id){
 $input=Find-Control $h '' $id 'Edit|Document' 'state'
 Assert-Proof ($input.focused -and $input.enabled -and !(($input.value+'').Trim())) "Manual input did not open empty and focused: $($input|ConvertTo-Json -Compress)"
}
$dash=Open-Dashboard
$openingPath=$script:lastTray
Settings-Shortcuts $dash
Enter-Text $dash 'shortcut-toolbox' 'Ctrl+Shift+K'
Click-Control $dash '' 'shortcuts-save'
[void](Wait-Proof 'valid new shortcut persisted' {try{(Get-Content -LiteralPath $cfg -Raw|ConvertFrom-Json).global_shortcuts.toolbox -ceq 'Ctrl+Shift+K'}catch{$false}} 12)
Assert-Saved 'Ctrl+Shift+K'
# Without closing dashboard or tray, new chord returns keyboard focus to the same product HWND.
Focus-Fixture
$reopened=Open-Shortcut @(17,16,75)
Assert-Proof ($reopened -eq $dash) 'Reopening Tool Box spawned a second visible window'
Settings-Shortcuts $dash
Enter-Text $dash 'shortcut-grammar' 'Ctrl+Shift+K'
Click-Control $dash '' 'shortcuts-save'
Shortcut-Status $dash '(?i)Phím tắt bị trùng'
Assert-Saved 'Ctrl+Shift+K'
Enter-Text $dash 'shortcut-grammar' 'Ctrl+Alt+G'
Enter-Text $dash 'shortcut-toolbox' 'Ctrl+Shift+J'
Click-Control $dash '' 'shortcuts-save'
Shortcut-Status $dash '(?i)Windows không đăng ký được'
Assert-Saved 'Ctrl+Shift+K'
Focus-Fixture
Assert-Proof ((Open-Shortcut @(17,16,75)) -eq $dash) 'A failed transaction did not restore prior live registration'
Close-Dashboard $dash;Stop-OwnedTray;Start-Tray
Focus-Fixture
$dash=Open-Shortcut @(17,16,75)
Settings-Shortcuts $dash
$box=Find-Control $dash '' 'shortcut-toolbox' 'Edit' 'state'
Assert-Proof ($box.value -ceq 'Ctrl+Shift+K') 'Custom shortcut did not display after Windows tray restart'
Capture-Evidence $dash 'shortcuts.png'
Close-Dashboard $dash
Write-Host "TOOLS-WIN-1: tray via $openingPath; configurable chord worked before/after restart and focused existing dashboard; UI rejects duplicate and actual OS-owned chord, leaving persistent and live settings intact."

Focus-Fixture $true
Set-Clipboard -Value 'selected-text-clipboard-sentinel'
Send-Keys @(17,18,84)
$quick=Product-Window $popupClass 'selected-text Translate quick popup'
$prefilled=Find-Control $quick '' 'translation-in' 'Edit' 'state'
Assert-Proof ($prefilled.value -ceq 'FixtureInputAlpha') 'Translate popup failed to prefill physical selection'
[void](Wait-Proof 'selected text was sent automatically' {@(Request-Log|Where-Object {$_.task -ceq 'translate' -and $_.source -ceq 'FixtureInputAlpha'}).Count -ge 1} 15)
Assert-Proof ((Get-Clipboard -Raw) -ceq 'selected-text-clipboard-sentinel') 'Selected-text launch touched the OS clipboard'
Prove-Copy $quick 'translation-copy' 'VI FixtureInputAlpha'
Start-Sleep -Milliseconds 500
Require-Requests 'translate' 'FixtureInputAlpha'
Capture-Evidence $quick 'selected-translate.png'
# Explicit app control routes to the regular Tool Box dashboard. It must not require a global key.
Click-Control $quick '' 'quick-open-toolbox'
$dash=Product-Window $dashClass 'popup Open Tool Box control'
[void](Wait-Proof 'explicit Tool Box action foreground' {[ProofWin]::GetForegroundWindow() -eq $dash} 10)
if(Visible $popupClass){Close-Quick $quick $popupClass;Foreground $dash}
Click-Control $dash '^Trợ lý AI$'
Click-Control $dash '^Translation$'
Enter-Text $dash 'translation-in' 'DashboardDraftAlpha'
Click-Control $dash '' 'translation-run'
Prove-Copy $dash 'translation-copy' 'VI DashboardDraftAlpha'
Require-Requests 'translate' 'DashboardDraftAlpha'
Click-Control $dash '^Grammar$'
Enter-Text $dash 'grammar-in' 'DashboardDraftAlpha'
Click-Control $dash '' 'grammar-run'
Prove-Copy $dash 'grammar-copy' 'Fixed DashboardDraftAlpha'
Require-Requests 'grammar' 'DashboardDraftAlpha'
Close-Dashboard $dash
Focus-Fixture
Send-Keys @(17,18,84)
$quick=Product-Window $popupClass 'manual Translate quick popup'
[void](Wait-Proof 'manual selected-text popup foreground' {[ProofWin]::GetForegroundWindow() -eq $quick} 8)
Assert-Focus-Input $quick 'translation-in'
Require-Requests 'translate' 'FixtureInputAlpha'
Enter-Text $quick 'translation-in' 'ManualDraftAlpha'
Click-Control $quick '' 'translation-run'
Prove-Copy $quick 'translation-copy' 'VI ManualDraftAlpha'
Require-Requests 'translate' 'ManualDraftAlpha'
Close-Quick $quick $popupClass
Write-Host 'TOOLS-WIN-2: selected Translate prefilled actual UIA selection, automatically submitted exactly once, did not read/write clipboard to extract selection, copied result, and both no-selection manual input / dashboard Translate worked.'

$quick=Capture-Crop
$ocr=Wait-Proof 'local stub received correctly sized physical PNG' {
 $o=@(Request-Log|Where-Object {$_.task -ceq 'ocr'});if($o.Count -eq 1){$o[0].result}
} 15
Prove-Copy $quick 'ocr-copy' $ocr
Click-Control $quick '^Dịch$'
Prove-Copy $quick 'ocr-translation-copy' ('VI '+$ocr)
Assert-Proof ((Visible $popupClass) -eq $quick -and !(Visible $dashClass) -and [ProofWin]::VisibleCount($popupClass,$dashClass) -eq 1) 'OCR translation did not remain inline in its native window'
Require-Requests 'ocr' 'native PNG'
Require-Requests 'translate' $ocr
Capture-Evidence $quick 'ocr-inline-translate.png'
Close-Quick $quick $popupClass
Write-Host 'TOOLS-WIN-3: physical OCR drag was anchored, product loopback submitted one PNG and one inline translation in the same HWND; native clipboard independently had the OCR result and translation.'

Focus-Fixture $true
Send-Keys @(17,18,71)
$quick=Product-Window $popupClass 'Grammar quick popup'
Prove-Copy $quick 'grammar-copy' 'Fixed FixtureInputAlpha'
Require-Requests 'grammar' 'FixtureInputAlpha'
Assert-Float $quick
Capture-Evidence $quick 'grammar-with-external-focus.png'
# Test the physical title bar close independently of Escape. UTF-8 is read by pwsh.
Click-Control $quick '^(✕|đóng|Đóng)$' 'window-close'
[void](Wait-Proof 'explicit close dismisses sticky Grammar' {!(Visible $popupClass)} 10)
Focus-Fixture
Send-Keys @(17,18,71)
$quick=Product-Window $popupClass 'Grammar popup reopen after explicit close'
Assert-Focus-Input $quick 'grammar-in'
Close-Quick $quick $popupClass
Write-Host 'TOOLS-WIN-4: Grammar worked from the real dashboard and physical global hotkey, stayed visible over real overlapping regular window without stealing focus, and explicit Close and Escape both dismiss it.'
