# push-to-github.ps1 -- push this project to GitHub and trigger the cloud build
#
# NOTE: intentionally ASCII-only, same reason as build.ps1 -- Windows PowerShell
# 5.1 reads .ps1 as ANSI unless it has a UTF-8 BOM, so non-ASCII text here would
# turn into mojibake. Chinese instructions live in README.md and in the chat.
#
# WHY SSH AND NOT HTTPS (measured on this machine):
#   github.com:443   -> TCP timeout, blocked by the network
#   github.com:22    -> reachable (~1s)
#   api.github.com   -> reachable
#   A Clash proxy runs on 127.0.0.1:7897 but git is not configured to use it.
#   So HTTPS pushes fail with "Failed to connect to github.com:443" while SSH
#   pushes succeed with the existing ~/.ssh/id_ed25519 key. Hence SSH below.
#
# Usage:
#   1. Create an EMPTY PUBLIC repo at https://github.com/new
#      Do NOT tick Add README / .gitignore / license -- it must be empty.
#   2. Run:
#        powershell -ExecutionPolicy Bypass -File .\push-to-github.ps1
#   3. Requires ~/.ssh/id_ed25519 to be registered on GitHub (already is).

[CmdletBinding()]
param(
    [string]$User = 'ismoneky',
    [string]$Repo = 'proc-cleaner'
)

Set-Location -LiteralPath $PSScriptRoot

$url = "git@github.com:$User/$Repo.git"
Write-Host "==> target: $url" -ForegroundColor Cyan

# NOTE on style: every git call goes through cmd /c on purpose.
# With $ErrorActionPreference = 'Stop', PowerShell turns ANY stderr output from a
# native command into a terminating error -- including harmless ones like git's
# "No such remote 'origin'". Routing through cmd keeps git's own exit codes
# authoritative and stops those messages from killing the script.

# --- 1. local commit must exist -------------------------------------------
cmd /c "git rev-parse --verify HEAD >nul 2>nul"
if ($LASTEXITCODE -ne 0) {
    Write-Host "[!!] No local commit yet." -ForegroundColor Red
    exit 1
}

# --- 2. point origin at the right URL -------------------------------------
$prevEap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$existing = cmd /c "git remote get-url origin 2>nul"
$ErrorActionPreference = $prevEap

if ($existing) {
    if ($existing -ne $url) {
        Write-Host "    updating origin: $existing -> $url"
        cmd /c "git remote set-url origin `"$url`""
    } else {
        Write-Host "    origin already correct"
    }
} else {
    cmd /c "git remote add origin `"$url`""
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[!!] Failed to add origin." -ForegroundColor Red
        exit 1
    }
    Write-Host "    added origin"
}

# --- 3. the remote must exist and be empty --------------------------------
# Non-interactive SSH here: if the key is not accepted we want a fast, clear
# failure rather than a hanging password prompt.
$env:GIT_SSH_COMMAND = "ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=20 -o BatchMode=yes"

Write-Host "==> checking remote ..." -ForegroundColor Cyan
$lsRemote = cmd /c "git ls-remote --heads origin 2>&1"
$lsCode = $LASTEXITCODE

if ($lsCode -ne 0) {
    Write-Host "[!!] Cannot reach $url" -ForegroundColor Red
    Write-Host "     Check that the repo exists on the web and is Public."
    Write-Host "     Check your SSH key is registered: ssh -T git@github.com"
    Write-Host "     Raw output: $lsRemote"
    exit 1
}

if ($lsRemote) {
    Write-Host "[!!] Remote is NOT empty. Existing refs:" -ForegroundColor Red
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
# Keep BatchMode: the ed25519 key needs no passphrase prompt, and if it ever
# does, failing loudly beats hanging forever on an invisible prompt.
Write-Host "==> pushing over SSH ..." -ForegroundColor Cyan
cmd /c "git push -u origin main"
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
