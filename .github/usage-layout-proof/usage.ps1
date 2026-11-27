# Native UIA plus a raw screenshot of only our dashboard's client area in a synthetic Windows runner profile.
# No JS or native test feature is injected into the product, and no credentials are present.
function Set-Dashboard-Size([IntPtr]$h,[int]$logical,[int]$height){
 $dpi=[ProofWin]::GetDpiForWindow($h)
 Assert-Proof ($dpi -gt 0) 'Missing native dashboard DPI'
 $targetWidth=[int][Math]::Round($logical*$dpi/96.0)
 $area=[ProofWin]::WorkAt(110,110)
 $x=$area.Left+16;$y=$area.Top+14
 $height=[Math]::Min($height,$area.Bottom-$y-12)
 Assert-Proof ($targetWidth -le $area.Right-$x-8 -and $height -ge 500) 'Windows test screen too small for requested physical sample'
 Assert-Proof ([ProofWin]::SetWindowPos($h,[IntPtr]::Zero,$x,$y,$targetWidth,$height,0x0014)) "Cannot resize dashboard to $logical logical px"
 [void](Wait-Proof "$logical logical px live HWND" {
  $r=[ProofWin+Rect]::new()
  [ProofWin]::GetWindowRect($h,[ref]$r) -and [Math]::Abs(($r.Right-$r.Left)-$targetWidth) -le 2
 } 8)
}
function Assert-Usage-UIA([IntPtr]$h,[int]$logical){
 $nodes=@((Uia $h '' '' '' 'list').items | Where-Object {(!$_.offscreen) -and $_.right -gt $_.left -and $_.bottom -gt $_.top})
 $area=[ProofWin+Rect]::new()
 Assert-Proof ([ProofWin]::GetWindowRect($h,[ref]$area)) 'No native Usage bounds'
 $observed=@($nodes|Where-Object {$_.name -cmatch '^(5 giờ|7 ngày)$' -and $_.left -ge $area.Left -and $_.right -le $area.Right -and $_.top -ge $area.Top -and $_.bottom -le $area.Bottom})
 $left=@($observed | Where-Object {$_.name -ceq '5 giờ'} | Sort-Object Top,Left | Select-Object -First 1)
 $right=@($observed | Where-Object {$_.name -ceq '7 ngày'} | Sort-Object Top,Left | Select-Object -First 1)
 Assert-Proof ($left.Count -eq 1 -and $right.Count -eq 1 -and $left[0].right -lt $right[0].left -and [Math]::Abs($left[0].top-$right[0].top) -le 5) "$logical px: first pair of quota labels not visibly side by side in actual Windows dashboard"
 Assert-Proof (@($nodes|Where-Object {$_.name -match '^96%$'}).Count -gt 0) 'Expected visible numeric 96% threshold quota'
 Assert-Proof (@($nodes|Where-Object {$_.name -match '^Dữ liệu cũ$'}).Count -gt 0) 'Expected visible stale state, distinct from quota bands'
 Assert-Proof (@($nodes|Where-Object {$_.name -match '^HTTP 429$'}).Count -gt 0) 'Expected visible current error detail'
 Assert-Proof (@($nodes|Where-Object {$_.name -match '(?i)^(thử lại|refresh|làm mới)' }).Count -gt 0) 'Missing visible Usage controls'
 Write-Host ("USAGE-UIA: " + (@{logical=$logical;left=$left[0];right=$right[0]; visible=@($nodes|Where-Object {$_.name -match 'Fixture|96%|Dữ liệu|HTTP 429' }|Select-Object -First 24)}|ConvertTo-Json -Compress -Depth 5))
}
function Capture-Usage([IntPtr]$h,[string]$path){
 Foreground $h
 $r=[ProofWin+Rect]::new();Assert-Proof ([ProofWin]::GetWindowRect($h,[ref]$r)) 'Missing owned dashboard rect for proof image'
 foreach($fx in @(0.02,0.25,0.5,0.75,0.98)){
  foreach($fy in @(0.02,0.25,0.5,0.75,0.98)){
   $xx=[int]($r.Left+($r.Right-$r.Left-1)*$fx);$yy=[int]($r.Top+($r.Bottom-$r.Top-1)*$fy)
   Assert-Proof ((Root-At $xx $yy) -eq $h) 'Refusing to capture a dashboard screenshot overlapped by another app'
  }
 }
 Capture-Evidence $h $path
 $file=Join-Path $evidence $path
 $hash=(Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash
 $data64=[Convert]::ToBase64String([IO.File]::ReadAllBytes($file))
 Write-Host "USAGE-SNAPSHOT: crop=$path sha256=$hash physical=$($r.Right-$r.Left)x$($r.Bottom-$r.Top)"
 Write-Host "::group::USAGE-SCREENSHOT:$path"
 for($i=0;$i -lt $data64.Length;$i+=720){Write-Host ("USAGE-PNG:"+$data64.Substring($i,[Math]::Min(720,$data64.Length-$i)))}
 Write-Host "USAGE-SCREENSHOT-END:$path"
 Write-Host '::endgroup::'
}
$data=Isolated-Environment 'usage-layout'
$now=[DateTimeOffset]::UtcNow
$epoch=$now.ToUnixTimeSeconds()
$threshold='Fixture Critical'
$stale='Fixture Throttled'
$regular='Fixture Team-Account-ABCDEFGHIJKLMNOPQRSTUVWXYZ-Extended-Name'
$profiles=@()
$cache=[ordered]@{}
$rootProfile=Join-Path $env:USERPROFILE 'synthetic-profiles'
New-Item -ItemType Directory -Path $rootProfile -Force|Out-Null
$initial=@{
 $threshold=@{five=96;week=63;fiveHours=2;weekHours=38}
 $stale=@{five=52;week=81;fiveHours=3;weekHours=95;err='HTTP 429';status=429}
 $regular=@{five=24;week=31;fiveHours=1;weekHours=141}
}
$i=0
foreach($name in @($regular,$stale,$threshold)){
 $i++
 $path=Join-Path $rootProfile "account-$i"
 New-Item -ItemType Directory -Path $path -Force|Out-Null
 $profiles+=@{name=$name;path=$path}
 $fixtureValues=$initial[$name]
 $cache[$name]=[ordered]@{
  path=$path;five=$fixtureValues.five;week=$fixtureValues.week
  five_reset=$now.AddHours($fixtureValues.fiveHours).ToString('o')
  week_reset=$now.AddHours($fixtureValues.weekHours).ToString('o')
  fetched_at=if($name -ceq $stale){$epoch-1020}else{$epoch-20}
  retry_at=if($name -ceq $stale){$epoch+600}else{0}
  err=$fixtureValues.err;status=$fixtureValues.status
 }
}
@{profiles=$profiles;autostart=$false;interval_sec=180;widget_enabled=$false;notify_enabled=$false;token_refresh=$false;block_enabled=$true;warn_pct=80;block_pct=95}|
 ConvertTo-Json -Depth 8|Set-Content -Encoding utf8 (Join-Path $data 'config.json')
@{ts=$epoch;block_enabled=$true;warn_pct=80;block_pct=95;busy=@();profiles=$cache}|
 ConvertTo-Json -Depth 8|Set-Content -Encoding utf8 (Join-Path $data 'usage_cache.json')

# Launch ordinary dashboard with production WebView, directly to keep the synthetic cache
# quiescent. Native tray + physical keys are independently exercised by the remap job.
$manual=Start-Process $realDashboard -PassThru
try{
 $dash=Product-Window $dashClass 'existing synthetic Usage Dashboard'
 Foreground $dash
 $nav=Find-Control $dash '^Claude Usage$' '' 'Button' 'point'
 $nav|Add-Member -NotePropertyName source -NotePropertyValue 'uia-screen'
 Button-Click $nav $dash
 [void](Find-Control $dash '^Fixture Critical$' '' '' 'state')
 [void](Find-Control $dash '^Fixture Throttled$' '' '' 'state')
 [void](Find-Control $dash '^Fixture Team-Account-' '' '' 'state')
 foreach($spec in @(@{width=780;file='claude-usage-normal.png'},@{width=420;file='claude-usage-narrow-420.png'})){
  Set-Dashboard-Size $dash $spec.width 690
  Start-Sleep -Milliseconds 450
  Assert-Usage-UIA $dash $spec.width
  Capture-Usage $dash $spec.file
 }
 Close-Dashboard $dash
 Write-Host 'USAGELAYOUT-1/2: real Windows desktop dashboard at logical 780/420 px shows paired quota controls, status and refresh/error detail from synthetic data; exact PNGs captured from the dashboard HWND only.'
}finally{
 if($manual -and !$manual.HasExited){Stop-Process -Id $manual.Id -Force -ErrorAction SilentlyContinue}
}
