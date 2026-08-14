# Veroeffentlicht Aura auf GitHub: Repository anlegen, pushen, Release bauen.
#
#   powershell -ExecutionPolicy Bypass -File publish.ps1
#
# Der einzige Schritt, der dich braucht, ist die Anmeldung bei GitHub - dabei
# oeffnet sich dein Browser und du bestaetigst einen Code. Danach laeuft alles
# automatisch: Repository anlegen, Code hochladen, Tag setzen. Die GitHub-Action
# baut daraus das Windows-Paket und haengt es an das Release; genau dieses Paket
# holt sich der eingebaute Auto-Updater.

Set-Location $PSScriptRoot

function Step($text) { Write-Host "`n== $text" -ForegroundColor Cyan }
function Fail($text) { Write-Host "`n$text" -ForegroundColor Red; exit 1 }

# ------------------------------------------------------------ gh.exe finden
# winget legt gh unter Program Files ab. Der Maschinen-PATH kennt das zwar,
# aber bereits offene Konsolen haben den alten PATH - deshalb direkt suchen.
$ghCandidates = @(
    "$env:ProgramFiles\GitHub CLI\gh.exe",
    "${env:ProgramFiles(x86)}\GitHub CLI\gh.exe",
    "$env:LOCALAPPDATA\Programs\GitHub CLI\gh.exe"
)
$gh = $ghCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $gh) {
    $cmd = Get-Command gh -ErrorAction SilentlyContinue
    if ($cmd) { $gh = $cmd.Source }
}
if (-not $gh) {
    Write-Host "GitHub CLI fehlt. Installiere sie mit:" -ForegroundColor Yellow
    Write-Host "  winget install --id GitHub.cli"
    Fail "Danach dieses Skript erneut starten."
}

# Native Programme schreiben Fortschritt nach stderr; PowerShell 5.1 macht daraus
# Fehler. Deshalb hier gebuendelt aufrufen und nur den Exit-Code auswerten.
function Invoke-Native {
    param([string]$Exe, [string[]]$Args, [switch]$Quiet)
    $out = & $Exe @Args 2>&1
    if (-not $Quiet) { $out | ForEach-Object { Write-Host "   $_" } }
    return @{ Code = $LASTEXITCODE; Output = ($out -join "`n") }
}

# ---------------------------------------------------------------- 1. Anmeldung
Step "GitHub-Anmeldung"
$status = Invoke-Native $gh @('auth', 'status') -Quiet
if ($status.Code -ne 0) {
    Write-Host "Es oeffnet sich gleich dein Browser." -ForegroundColor Yellow
    Write-Host "Waehle 'Login with a web browser', kopiere den Code, fertig.`n"
    & $gh auth login --hostname github.com --git-protocol https --web
    $status = Invoke-Native $gh @('auth', 'status') -Quiet
    if ($status.Code -ne 0) { Fail "Anmeldung nicht abgeschlossen - Skript erneut starten." }
}
Invoke-Native $gh @('auth', 'setup-git') -Quiet | Out-Null
$user = (& $gh api user --jq .login 2>$null | Select-Object -First 1)
if (-not $user) { Fail "Konnte den Benutzernamen nicht lesen." }
$user = $user.Trim()
Write-Host "Angemeldet als $user" -ForegroundColor Green

# ------------------------------------------------- 2. Repo-Namen im Code fixen
$repo = "$user/aura-browser"
Step "Auto-Updater auf $repo zeigen lassen"
$updateRs = "src/update.rs"
$content = Get-Content $updateRs -Raw
$wanted = 'pub const REPO: &str = "' + $repo + '";'
if ($content -notmatch [regex]::Escape($wanted)) {
    $content = $content -replace 'pub const REPO: &str = "[^"]*";', $wanted
    [System.IO.File]::WriteAllText((Resolve-Path $updateRs), $content, (New-Object System.Text.UTF8Encoding($false)))
    Invoke-Native 'git' @('add', $updateRs) -Quiet | Out-Null
    Invoke-Native 'git' @('commit', '-q', '-m', "Auto-Updater auf $repo zeigen lassen") -Quiet | Out-Null
    Write-Host "   angepasst"
} else {
    Write-Host "   schon korrekt"
}

# ---------------------------------------------------------------- 3. Repository
Step "Repository anlegen"
$exists = Invoke-Native $gh @('repo', 'view', $repo) -Quiet
if ($exists.Code -ne 0) {
    $create = Invoke-Native $gh @(
        'repo', 'create', $repo, '--public', '--source=.', '--remote=origin', '--push',
        '--description', 'Nativer Windows-Browser in Rust - ohne Electron, mit eigenem Adblocker'
    )
    if ($create.Code -ne 0) { Fail "Repository konnte nicht angelegt werden." }
} else {
    Write-Host "   existiert bereits, pushe nur"
    $remotes = (& git remote 2>$null) -join ' '
    if ($remotes -notmatch 'origin') {
        Invoke-Native 'git' @('remote', 'add', 'origin', "https://github.com/$repo.git") -Quiet | Out-Null
    }
    $push = Invoke-Native 'git' @('push', '-u', 'origin', 'main')
    if ($push.Code -ne 0) { Fail "Push fehlgeschlagen." }
}

# ------------------------------------------------------------------- 4. Themen
Step "Themen setzen"
Invoke-Native $gh @(
    'repo', 'edit', $repo,
    '--add-topic', 'browser', '--add-topic', 'rust', '--add-topic', 'windows',
    '--add-topic', 'webview2', '--add-topic', 'adblocker', '--add-topic', 'direct2d'
) -Quiet | Out-Null

# ------------------------------------------------------------------ 5. Release
$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$tag = "v$version"
Step "Release $tag"
$hasTag = (& git tag -l $tag 2>$null) -join ''
if (-not $hasTag) {
    Invoke-Native 'git' @('tag', '-a', $tag, '-m', "Aura Browser $version") -Quiet | Out-Null
    Write-Host "   Tag $tag angelegt"
} else {
    Write-Host "   Tag existiert bereits"
}
$pushTag = Invoke-Native 'git' @('push', 'origin', $tag)
if ($pushTag.Code -ne 0) { Fail "Tag konnte nicht gepusht werden." }

Write-Host "`nFertig." -ForegroundColor Green
Write-Host "Repository:  https://github.com/$repo"
Write-Host "Actions:     https://github.com/$repo/actions"
Write-Host "Das Paket erscheint in ein paar Minuten unter Releases."
