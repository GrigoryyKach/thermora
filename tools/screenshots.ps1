# Makes the README screenshots with the demo data (fake readings and a fake PC, nothing real).
# Usage: .\tools\screenshots.ps1 [-Exe target\release\Thermora.exe]
param([string]$Exe = "$PSScriptRoot\..\target\release\Thermora.exe")

$out = Join-Path $PSScriptRoot '..\assets\screenshots'
New-Item -ItemType Directory -Force $out | Out-Null
$tmp = Join-Path $env:TEMP "thermora-shots-$PID"
New-Item -ItemType Directory -Force $tmp | Out-Null
$env:THERMORA_DATA_DIR = $tmp
$env:THERMORA_WINDOW = '1280x940'
$env:THERMORA_SHOT_WAIT = '60'

function Shoot($theme, $lang, $navs, $select, $prefixExtra) {
    $env:THERMORA_THEME = $theme
    $env:THERMORA_LANG = $lang
    $env:THERMORA_SHOT_NAVS = $navs
    $env:THERMORA_SHOT_PREFIX = "$theme-$lang"
    $env:THERMORA_SHOT_DIR = $tmp
    if ($select -ne $null) { $env:THERMORA_SHOT_SELECT = "$select" } else { Remove-Item Env:THERMORA_SHOT_SELECT -ErrorAction SilentlyContinue }
    Start-Process -FilePath (Resolve-Path $Exe) -ArgumentList "--demo" -Wait
    Get-ChildItem $tmp -Filter "$theme-$lang-*.png" | ForEach-Object { Move-Item $_.FullName (Join-Path $out $_.Name) -Force }
}

Shoot 'dark' 'en' 'monitor,specs,settings' 0
Shoot 'light' 'en' 'monitor,specs' 5
Shoot 'dark' 'ru' 'monitor,specs' 0
Shoot 'dark' 'uk' 'monitor' $null

# The state before the CPU temperature access is enabled: the banner, then the consent dialog.
$env:THERMORA_DEMO_NOACCESS = '1'
Shoot 'dark' 'en' 'monitor' $null
Move-Item (Join-Path $out 'dark-en-monitor.png') (Join-Path $out 'dark-en-monitor-banner.png') -Force
$env:THERMORA_SHOT_DIALOG = '1'
Shoot 'dark' 'en' 'monitor' $null
Move-Item (Join-Path $out 'dark-en-monitor.png') (Join-Path $out 'dark-en-dialog.png') -Force
Remove-Item Env:THERMORA_DEMO_NOACCESS, Env:THERMORA_SHOT_DIALOG -ErrorAction SilentlyContinue

# Restore the plain dark English shots (the banner run overwrote the name).
Shoot 'dark' 'en' 'monitor' $null
Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
