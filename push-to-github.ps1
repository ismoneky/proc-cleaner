# push-to-github.ps1 -- push this project to GitHub and trigger the cloud build
#
# NOTE: intentionally ASCII-only, same reason as build.ps1 -- Windows PowerShell
# 5.1 reads .ps1 as ANSI unless it has a UTF-8 BOM, so non-ASCII text here would
# turn into mojibake. Chinese instructions live in README.md and in the chat.
#
# Usage:
#   1. Create an EMPTY PUBLIC repo named "proc-cleaner" at https://github.com/new
#      Do NOT tick Add README / .gitignore / license -- it must be empty.
#   2. Run:
#        powershell -ExecutionPolicy Bypass -File .\push-to-github.ps1
#   3. A browser window pops up for GitHub login. No token needs to be typed.

[CmdletBinding()]
param(
    [string]$User = 'ismoneky',
    [string]$Repo = 'proc-cleaner'
)

$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot

$url = "https://github.com/$User/$Repo.git"

Write-Host "==> target: $url" -ForegroundColor Cyan

# --- 1. local commit must exist -------------------------------------------
git rev-parse --verify HEAD *> $null
if ($LASTEXITCODE -ne 0) {
    Write-Host "[!!] No local commit yet." -ForegroundColor Red
    exit 1
}

# --- 2. point origin at the right URL -------------------------------------
$existing = git remote get-url origin 2>$null
if ($LASTEXITCODE -eq 0 -and $existing) {
    if ($existing -ne $url) {
        Write-Host "    updating origin: $existing -> $url"
        git remote set-url origin $url
    } else {
        Write-Host "    origin already correct"
    }
} else {
    git remote add origin $url
    Write-Host "    added origin"
}

# --- 3. the remote must exist and be empty --------------------------------
Write-Host "==> checking remote ..." -ForegroundColor Cyan
$lsRemote = git ls-remote --heads origin 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host "[!!] Cannot reach $url" -ForegroundColor Red
    Write-Host "     Check that the repo exists on the web and is Public."
    Write-Host "     Raw error: $lsRemote"
    exit 1
}

if ($lsRemote) {
    Write-Host "[!!] Remote is NOT empty. Existing branches:" -ForegroundColor Red
    Write-Host $lsRemote
    Write-Host ""
    Write-Host "     We have a local commit, so a plain push would conflict."
    Write-Host "     Option A: delete the remote repo, create a new empty one, rerun."
    Write-Host "     Option B: if the remote content is disposable, force push:"
    Write-Host "                 git push -u origin main --force"
    exit 1
}

Write-Host "    remote is empty, ready to push" -ForegroundColor Green

# --- 4. push --------------------------------------------------------------
Write-Host "==> pushing (a GitHub login window will pop up) ..." -ForegroundColor Cyan
git push -u origin main
if ($LASTEXITCODE -ne 0) {
    Write-Host "[!!] Push failed. Paste the raw error above." -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "[ok] pushed successfully" -ForegroundColor Green
Write-Host ""
Write-Host "GitHub Actions has started building (3-8 min on a cold cache)."
Write-Host ""
Write-Host "Build progress:"
Write-Host "  https://github.com/$User/$Repo/actions"
Write-Host ""
Write-Host "Permanent download link once the build succeeds:"
Write-Host "  https://github.com/$User/$Repo/releases/latest/download/proc-cleaner.exe"
