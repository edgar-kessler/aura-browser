# Baut das fertige Setup: Browser -> Nutzlast -> AuraBrowserSetup-<Version>.exe
#
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1 -SkipBrowser   (Browser ist schon gebaut)
#
# Der Installer selbst ist ein Rust-Programm (installer/); die zu installierenden
# Dateien haengt er sich mit `pack` hinten an. Ergebnis landet in dist\.
param(
    [switch]$SkipBrowser,
    [string]$Version
)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

function Step($text) { Write-Host "`n== $text" -ForegroundColor Cyan }

if (-not $Version) {
    $Version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
}

if (-not $SkipBrowser) {
    Step "Browser bauen"
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "Browser-Build fehlgeschlagen" }
}

Step "Installer bauen"
cargo build --release -p aura-setup
if ($LASTEXITCODE -ne 0) { throw "Installer-Build fehlgeschlagen" }

Step "Nutzlast zusammenstellen"
$payload = Join-Path 'dist' 'payload'
if (Test-Path $payload) { Remove-Item -Recurse -Force $payload }
New-Item -ItemType Directory -Force -Path $payload | Out-Null
Copy-Item 'target\release\aura-browser.exe', 'target\release\WebView2Loader.dll', 'LICENSE', 'README.md' $payload
Copy-Item -Recurse 'assets' (Join-Path $payload 'assets')

Step "Setup packen"
$out = Join-Path 'dist' "AuraBrowserSetup-$Version.exe"
$p = Start-Process -FilePath 'target\release\aura-setup.exe' `
    -ArgumentList @('pack', '--payload', $payload, '--out', $out, '--version', $Version) `
    -Wait -NoNewWindow -PassThru
if ($p.ExitCode -ne 0) { throw "Packen fehlgeschlagen (Code $($p.ExitCode))" }

Get-Item $out | Select-Object Name, @{n = 'MB'; e = { [math]::Round($_.Length / 1MB, 2) } }
