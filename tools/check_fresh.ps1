$repo = Split-Path -Parent $PSScriptRoot
$target = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).target_directory
$exe = Get-Item (Join-Path $target 'release\dt-ocr.exe')
$src = Get-ChildItem -Path (Join-Path $repo 'dt-ocr\src'), (Join-Path $repo 'dt-ocr\scripts'), (Join-Path $repo 'dt-common\src') -Recurse -File |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($exe.LastWriteTime -gt $src.LastWriteTime) { 'FRESH: exe newer than all sources' } else { 'STALE'; exit 1 }
Get-Content -LiteralPath (Join-Path $target 'release\dt-ocr.log') -Tail 3 -Encoding UTF8
