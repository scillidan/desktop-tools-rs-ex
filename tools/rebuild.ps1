param([switch]$Clean)
$repo = Split-Path -Parent $PSScriptRoot
$target = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).target_directory
$exe = Join-Path $target 'release\dt-ocr.exe'
taskkill /F /IM dt-ocr.exe 2>$null | Out-Null
Start-Sleep -Milliseconds 800
Push-Location $repo
if ($Clean) { cargo clean }
cargo build --release
if ($LASTEXITCODE -ne 0) { Pop-Location; Write-Host 'BUILD FAILED'; exit 1 }
Pop-Location
Copy-Item (Join-Path $repo 'dt-ocr.toml') (Join-Path $target 'release\dt-ocr.toml') -Force
$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = "`"$exe`"" }
Write-Host "start ret=$($r.ReturnValue) pid=$($r.ProcessId)"
Start-Sleep -Seconds 1
$alive = Get-CimInstance -ClassName Win32_Process -Filter "Name='dt-ocr.exe'"
if (-not $alive) { 'NOT RUNNING (BAD)'; exit 1 }
"alive: pid=$($alive.ProcessId)"
$sig = @'
[DllImport("user32.dll", SetLastError = true)]
public static extern bool RegisterHotKey(System.IntPtr hWnd, int id, uint fsModifiers, uint vk);
[DllImport("user32.dll", SetLastError = true)]
public static extern bool UnregisterHotKey(System.IntPtr hWnd, int id);
'@
$api = Add-Type -MemberDefinition $sig -Name User32Probe -Namespace DtOcr -PassThru
if ($api::RegisterHotKey([IntPtr]::Zero, 99, 0x0003, 0x53)) {
    $api::UnregisterHotKey([IntPtr]::Zero, 99) | Out-Null
    Write-Warning "ctrl+alt+s still FREE - dt-ocr hotkeys not registered?"
    exit 1
}
"hotkey probe: ctrl+alt+s held (pid $($alive.ProcessId))"
