# Veroeffentlicht Aura auf GitHub: Repository anlegen, pushen, Release bauen.
#
#   powershell -ExecutionPolicy Bypass -File publish.ps1
#
# Der einzige Schritt, der dich braucht, ist die Anmeldung bei GitHub – dabei
# oeffnet sich dein Browser und du bestaetigst einen Code. Danach laeuft alles
# automatisch: Repository anlegen, Code hochladen, Tag setzen. Die GitHub-Action
# baut daraus das Windows-Paket und haengt es an das Release; genau dieses Paket
# holt sich der eingebaute Auto-Updater.

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$env:PATH = "$env:LOCALAPPDATA\Programs\GitHub CLI;$env:ProgramFiles\GitHub CLI;$env:PATH"

function Step($text) { Write-Host "`n== $text" -ForegroundColor Cyan }

# ---------------------------------------------------------------- 1. Anmeldung
Step "GitHub-Anmeldung"
gh auth status 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Host "Es oeffnet sich gleich dein Browser. Code eingeben, fertig." -ForegroundColor Yellow
    gh auth login --hostname github.com --git-protocol https --web
    if ($LASTEXITCODE -ne 0) { throw "Anmeldung abgebrochen." }
}
gh auth setup-git
$user = (gh api user --jq .login).Trim()
Write-Host "Angemeldet als $user"

# ------------------------------------------------- 2. Repo-Namen im Code fixen
$repo = "$user/aura-browser"
Step "Auto-Updater auf $repo zeigen lassen"
$updateRs = "src/update.rs"
$content = Get-Content $updateRs -Raw
$wanted = 'pub const REPO: &str = "' + $repo + '";'
if ($content -notmatch [regex]::Escape($wanted)) {
    $content = $content -replace 'pub const REPO: &str = "[^"]*";', $wanted
    Set-Content $updateRs $content -NoNewline -Encoding utf8
    git add $updateRs
    git commit -q -m "Auto-Updater auf $repo zeigen lassen"
    Write-Host "angepasst"
} else {
    Write-Host "schon korrekt"
}

# ---------------------------------------------------------------- 3. Repository
Step "Repository anlegen"
gh repo view $repo 2>$null | Out-Null
if ($LASTEXITCODE -ne 0) {
    gh repo create $repo --public --source=. --remote=origin --push `
        --description "Nativer Windows-Browser in Rust - ohne Electron, mit eigenem Adblocker"
} else {
    Write-Host "existiert bereits, pushe nur"
    if (-not (git remote | Select-String -Quiet origin)) {
        git remote add origin "https://github.com/$repo.git"
    }
    git push -u origin main
}

# ------------------------------------------------------------------- 4. Themen
Step "Themen setzen"
gh repo edit $repo --add-topic browser --add-topic rust --add-topic windows `
    --add-topic webview2 --add-topic adblocker --add-topic direct2d 2>$null

# ------------------------------------------------------------------ 5. Release
$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$tag = "v$version"
Step "Release $tag"
if (git tag -l $tag) {
    Write-Host "Tag existiert bereits"
} else {
    git tag -a $tag -m "Aura Browser $version"
}
git push origin $tag

Write-Host "`nFertig." -ForegroundColor Green
Write-Host "Repository:  https://github.com/$repo"
Write-Host "Actions:     https://github.com/$repo/actions"
Write-Host "Das Paket erscheint in ein paar Minuten unter Releases."
