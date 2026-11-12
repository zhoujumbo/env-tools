$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$releaseDir = Join-Path $root "src-tauri\target\release"
$exeName = "EnvtsPanel.exe"
$exeCandidates = @(
    (Join-Path $releaseDir $exeName),
    (Join-Path $releaseDir "envdesk.exe")
)
$exe = $exeCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $exe) {
    throw "Missing EnvtsPanel.exe / envdesk.exe. Run: npm run portable"
}

function Get-AppVersion {
    $pkg = Join-Path $root "package.json"
    if (Test-Path $pkg) {
        $raw = Get-Content $pkg -Raw
        if ($raw -match '"version"\s*:\s*"([^"]+)"') { return $Matches[1] }
    }
    return "0.0.0"
}

function Find-WebView2Loader {
    $direct = Join-Path $releaseDir "WebView2Loader.dll"
    if (Test-Path $direct) { return $direct }
    $hit = Get-ChildItem -Path (Join-Path $root "src-tauri\target") -Recurse -Filter "WebView2Loader.dll" -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\|\\win-x64\\|\\release\\' } |
        Select-Object -First 1
    if ($hit) { return $hit.FullName }
    return $null
}

$version = Get-AppVersion
$loader = Find-WebView2Loader
if (-not $loader) {
    throw "Missing WebView2Loader.dll next to the exe"
}

$outDir = Join-Path $root "dist-portable"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$stage = Join-Path $outDir "EnvtsPanel"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item $exe (Join-Path $stage $exeName)
Copy-Item $loader (Join-Path $stage "WebView2Loader.dll")
Get-ChildItem $releaseDir -Filter "*.dll" -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -ne "WebView2Loader.dll" } |
    ForEach-Object { Copy-Item $_.FullName (Join-Path $stage $_.Name) }

$readme = @"
EnvtsPanel portable $version
===================
1. Extract the folder. Keep EnvtsPanel.exe and WebView2Loader.dll together.
2. Double-click EnvtsPanel.exe
3. Optional: set user env ENVT_PATH=D:\your-data-dir
   Default data dir: %APPDATA%\envts
4. WebView2 Runtime is required (Win10/11 usually already has it).
   If missing: https://developer.microsoft.com/microsoft-edge/webview2/
"@
$utf8NoBom = New-Object System.Text.UTF8Encoding $false
[System.IO.File]::WriteAllText((Join-Path $stage "README.txt"), $readme, $utf8NoBom)

$zipName = "EnvtsPanel-$version-win-x64.zip"
$zip = Join-Path $outDir $zipName
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $stage -DestinationPath $zip -Force
Write-Host "OK: $zip"
Write-Host "version: $version"
Write-Host "exe: $exe"
Write-Host "loader: $loader"
Get-Item $zip | Format-List FullName, Length
