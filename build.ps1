# build.ps1 -- build proc-cleaner.exe
#
# NOTE: this script is intentionally ASCII-only. Windows PowerShell 5.1 reads
# .ps1 files as ANSI unless they carry a UTF-8 BOM, so non-ASCII text here would
# show up as mojibake on some machines. Chinese explanations live in README.md.
#
# Usage:
#   .\build.ps1                     # compile release, output dist\proc-cleaner.exe
#   .\build.ps1 -CheckOnly          # just run `cargo check`, fastest way to see if it compiles
#   .\build.ps1 -EmbedAdminManifest # also embed requireAdministrator (UAC prompt on launch)
#   .\build.ps1 -Run                # build, then start the exe

[CmdletBinding()]
param(
    [switch]$CheckOnly,
    [switch]$EmbedAdminManifest,
    [switch]$Run
)

$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot

function Write-Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Write-Ok($msg)   { Write-Host "[ok] $msg" -ForegroundColor Green }
function Write-Bad($msg)  { Write-Host "[!!] $msg" -ForegroundColor Red }

# ---------------------------------------------------------------- 0. toolchain
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Bad "cargo not found on PATH."
    Write-Host "    Install the Rust toolchain first: https://rustup.rs"
    Write-Host "    After installing, open a NEW terminal so PATH is refreshed."
    exit 1
}

$cargoVersion = (cargo --version) 2>&1
$rustcVersion = (rustc --version) 2>&1
Write-Host "    $cargoVersion"
Write-Host "    $rustcVersion"
Write-Host ""

# ---------------------------------------------------------------- 1. cargo check
Write-Step "1/4  cargo check --release"
cargo check --release
if ($LASTEXITCODE -ne 0) {
    Write-Bad "Code does not compile. Scroll up for the exact errors."
    Write-Host "    Nothing below this point will work until those are fixed."
    exit 1
}
Write-Ok "compiles cleanly."
Write-Host ""

if ($CheckOnly) {
    Write-Ok "-CheckOnly specified, stopping here."
    exit 0
}

# ------------------------------------------------- 2. optional admin manifest
# Cleared by default so the plain `cargo build` path stays simple and predictable.
$env:RUSTFLAGS = $null
$env:CARGO_ENCODED_RUSTFLAGS = $null

if ($EmbedAdminManifest) {
    $manifest = Join-Path $PSScriptRoot 'proc-cleaner.manifest'
    if (-not (Test-Path -LiteralPath $manifest)) {
        Write-Bad "proc-cleaner.manifest not found next to this script."
        exit 1
    }
    # CARGO_ENCODED_RUSTFLAGS uses 0x1F as the argument separator, which keeps a
    # path containing spaces as ONE argument (RUSTFLAGS would split it).
    $env:CARGO_ENCODED_RUSTFLAGS = "-Clink-arg=/MANIFESTINPUT:$manifest"
    Write-Host "    embedding requireAdministrator manifest (UAC prompt on launch)"
}
Write-Host ""

# ---------------------------------------------------------------- 3. build
Write-Step "2/4  cargo build --release"
cargo build --release
if ($LASTEXITCODE -ne 0) {
    Write-Bad "Build failed. If the failure mentions /MANIFESTINPUT, retry without -EmbedAdminManifest."
    exit 1
}
Write-Ok "build succeeded."
Write-Host ""

# ---------------------------------------------------------------- 4. collect
Write-Step "3/4  collecting artifact"
$src = Join-Path $PSScriptRoot 'target\release\proc-cleaner.exe'
if (-not (Test-Path -LiteralPath $src)) {
    Write-Bad "expected artifact not found: $src"
    exit 1
}

$dist = Join-Path $PSScriptRoot 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$dst = Join-Path $dist 'proc-cleaner.exe'
Copy-Item -LiteralPath $src -Destination $dst -Force

$sizeMb = [math]::Round((Get-Item -LiteralPath $dst).Length / 1MB, 2)
Write-Ok "dist\proc-cleaner.exe  ($sizeMb MB)"
Write-Host ""

# ---------------------------------------------------------------- 5. run
Write-Step "4/4  done"
Write-Host "    This exe is self-contained: no Python, no .NET, no runtime to install."
Write-Host "    Killing other users' processes requires elevation."
Write-Host "    Either build with -EmbedAdminManifest, or right-click the exe -> Run as administrator."

if ($Run) {
    Write-Host ""
    Write-Host "    starting $dst ..."
    Start-Process -FilePath $dst
}
