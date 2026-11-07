param([switch]$Clean)
$repo = Split-Path -Parent $PSScriptRoot
$target = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).target_directory
$exe = Join-Path $target 'release\screen-tools-rs.exe'
taskkill /F /IM screen-tools-rs.exe 2>$null | Out-Null
taskkill /F /IM dt-ocr.exe 2>$null | Out-Null
Start-Sleep -Milliseconds 800
Push-Location $repo
if ($Clean) { cargo clean }
cargo build --release
if ($LASTEXITCODE -ne 0) { Pop-Location; Write-Host 'BUILD FAILED'; exit 1 }
Pop-Location
$config = Join-Path $target 'release\screen-tools.toml'
if (-not (Test-Path $config)) {
    $legacy = Join-Path $target 'release\dt-ocr.toml'
    $source = if (Test-Path $legacy) { $legacy } else { Join-Path $repo 'screen-tools.toml' }
    Copy-Item $source $config
}
$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = "`"$exe`"" }
Write-Host "start ret=$($r.ReturnValue) pid=$($r.ProcessId)"
Start-Sleep -Seconds 1
$alive = Get-CimInstance -ClassName Win32_Process -Filter "Name='screen-tools-rs.exe'"
if (-not $alive) { 'NOT RUNNING (BAD)'; exit 1 }
"alive: pid=$($alive.ProcessId)"
$sig = @'
[DllImport("user32.dll", SetLastError = true)]
public static extern bool RegisterHotKey(System.IntPtr hWnd, int id, uint fsModifiers, uint vk);
[DllImport("user32.dll", SetLastError = true)]
public static extern bool UnregisterHotKey(System.IntPtr hWnd, int id);
'@
$api = Add-Type -MemberDefinition $sig -Name User32Probe -Namespace ScreenToolsRs -PassThru
$held = $false
for ($i = 0; $i -lt 10; $i++) {
    if (-not $api::RegisterHotKey([IntPtr]::Zero, 99, 0x0003, 0x53)) {
        $held = $true
        break
    }
    $api::UnregisterHotKey([IntPtr]::Zero, 99) | Out-Null
    Start-Sleep -Milliseconds 200
}
if (-not $held) {
    Write-Warning "ctrl+alt+s still FREE - screen-tools-rs hotkeys not registered?"
    exit 1
}
"hotkey probe: ctrl+alt+s held (pid $($alive.ProcessId))"
