# Actual UI save; settings values plus OS ownership probes and physical keys (no direct config edits or tray messages).
$bindings=[ordered]@{
 grammar=@{label='Ctrl+Shift+F6';mods=6;new=0x75;old=0x47;name='Grammar'}
 translation=@{label='Ctrl+Shift+F7';mods=6;new=0x76;old=0x54;name='Translate'}
 ocr=@{label='Ctrl+Shift+F8';mods=6;new=0x77;old=0x4F;name='OCR'}
}
function Require-Bindings([IntPtr]$h){
 $j=Get-Content -LiteralPath $cfg -Raw|ConvertFrom-Json
 Assert-Proof ($j.global_shortcuts.toolbox -ceq '' -and $j.old_extra_fixture -ceq 'preserve me') 'Remap changed unrelated preferences'
 foreach($key in $bindings.Keys){
  $item=$bindings[$key];$shown=Find-Control $h '' "shortcut-$key" 'Edit' 'state'
  Assert-Proof ($j.global_shortcuts.$key -ceq $item.label -and $shown.value -ceq $item.label) "$($item.name) was not persisted/displayed as $($item.label)"
 }
}
function Probe-Bindings{
 foreach($key in $bindings.Keys){
  $item=$bindings[$key]
  $statusOld=[ProofWin]::CheckRegistration(3,$item.old)
  $statusNew=[ProofWin]::CheckRegistration($item.mods,$item.new)
  # ERROR_HOTKEY_ALREADY_REGISTERED is specifically expected for the new chords, not an arbitrary return error.
  Assert-Proof ($statusOld -eq 0 -and $statusNew -eq 1409) "$($item.name): old/new Win32 registration probe returned $statusOld/$statusNew, expected 0/1409"
 }
}
function Assert-Old-Chords{
 Focus-Fixture $true
 foreach($key in $bindings.Keys){
  $item=$bindings[$key]
  $guard=[ProofWin+VisibilityWatch]::new($pickerClass,$popupClass)
  try{
    Start-Sleep -Milliseconds 85
    Send-Keys @(17,18,$item.old)
    Start-Sleep -Milliseconds 1500
    Assert-Proof (!$guard.Seen -and $guard.Samples -gt 30 -and !(Visible $popupClass) -and !(Visible $pickerClass) -and [ProofWin]::GetForegroundWindow() -eq $fixtureH) "Old $($item.name) chord still opened a tool or moved focus"
    Write-Host ('No native window for old '+$item.name+' chord during '+$guard.Samples+' physical samples')
  }finally{$guard.Dispose()}
 }
}
function Prove-New-Chords([string]$stage,[int]$expected){
 foreach($kind in @('grammar','translation')){
  $binding=$bindings[$kind];$tag=if($kind -eq 'grammar'){'grammar'}else{'translate'}
  Focus-Fixture $true
  Set-Clipboard -Value 'no-source-clipboard-write'
  Send-Keys @(17,16,$binding.new)
  $quick=Product-Window $popupClass "$stage $($binding.name) quick popup"
  $shown=Find-Control $quick '' "$kind-in" 'Edit' 'state'
  Assert-Proof ($shown.value -ceq 'FixtureInputAlpha') "Remapped $($binding.name) did not use actual external selection"
  Assert-Proof ((Get-Clipboard -Raw) -ceq 'no-source-clipboard-write') 'Reading selected text touched the clipboard'
  $text=if($kind -eq 'grammar'){'Fixed FixtureInputAlpha'}else{'VI FixtureInputAlpha'}
  Prove-Copy $quick "$kind-copy" $text
  Require-Requests $tag 'FixtureInputAlpha' $expected
  Assert-Proof (!(Visible $pickerClass) -and !(Visible $dashClass)) "$($binding.name) opened the wrong surface"
  Close-Quick $quick $popupClass
 }
 $quick=Capture-Crop @(17,16,$bindings.ocr.new)
 $ocr=Wait-Proof "$stage actual capture submitted a valid PNG" {
  $values=@(Request-Log|Where-Object {$_.task -ceq 'ocr'});if($values.Count -eq $expected){$values[-1].result}
 } 15
 Prove-Copy $quick 'ocr-copy' $ocr
 Require-Requests 'ocr' 'native PNG' $expected
 Close-Quick $quick $popupClass
 Write-Host "REMAPPROOF: $stage verified actual Ctrl+Shift+F6 Grammar selection + clipboard, Ctrl+Shift+F7 Translate selection + clipboard, and Ctrl+Shift+F8 OCR PNG physical crop + clipboard."
}
$dash=Open-Dashboard
Settings-Shortcuts $dash
foreach($key in $bindings.Keys){
 $item=$bindings[$key]
 Assert-Proof ([ProofWin]::CheckRegistration(3,$item.old) -eq 1409) "Default $($item.name) chord was not registered before UI save"
 Assert-Proof ([ProofWin]::CheckRegistration($item.mods,$item.new) -eq 0) "Chosen replacement was not initially available: $($item.label)"
 Enter-Text $dash "shortcut-$key" $item.label
}
Click-Control $dash '' 'shortcuts-save'
[void](Wait-Proof 'windows settings UI persisted all three replacement chords' {
 try{
  $j=Get-Content -LiteralPath $cfg -Raw|ConvertFrom-Json
  $j.global_shortcuts.grammar -ceq $bindings.grammar.label -and $j.global_shortcuts.translation -ceq $bindings.translation.label -and $j.global_shortcuts.ocr -ceq $bindings.ocr.label
 }catch{$false}
} 10)
Require-Bindings $dash
Probe-Bindings
Close-Dashboard $dash
Assert-Old-Chords
Prove-New-Chords 'after real save, before restart' 1
Stop-OwnedTray
foreach($key in $bindings.Keys){
 $item=$bindings[$key]
 Assert-Proof ([ProofWin]::CheckRegistration($item.mods,$item.new) -eq 0) "New chord did not release when tray quit: $($item.label)"
}
Start-Tray
$dash=Open-Dashboard
Settings-Shortcuts $dash
Require-Bindings $dash
Probe-Bindings
Capture-Evidence $dash 'remapped-tool-shortcuts.png'
Close-Dashboard $dash
Assert-Old-Chords
Prove-New-Chords 'after native tray restart' 2
Write-Host 'USAGELAYOUT-3 PASS: three remaps from real menu, settings/Win32 registration persisted, three old defaults released/inert, each replacement produced the right native behavior both before and after tray restart.'
