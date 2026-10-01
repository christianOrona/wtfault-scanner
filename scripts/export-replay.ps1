# Turn a session recorded on this machine into a replay fixture for CI (#59).
#
# USAGE
#
#   scripts\export-replay.ps1                                   # list sessions
#   scripts\export-replay.ps1 -Session <id> -Name 2019-f250-full-scan
#
# Reads a copy of the app's database, never the database itself, and writes
# core\diagnostics\tests\replays\<Name>.transcript with the VIN anonymised. It
# refuses to write anything if a trace of the real VIN is left. Then it records
# the new transcript's baseline: what the current core discovers replaying it.
#
# Close the app first, so the copy is not taken halfway through a write.
param(
    [string]$Session,
    [string]$Name,
    [string]$Db = (Join-Path $env:APPDATA "ai-mechanic\data\sessions.sqlite")
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
try {
    if (-not $Session) {
        cargo run -q -p aim-diagnostics --example export_replay -- $Db
        exit $LASTEXITCODE
    }
    if (-not $Name) {
        Write-Host "Give the fixture a name: -Name 2019-f250-full-scan" -ForegroundColor Red
        exit 2
    }

    cargo run -q -p aim-diagnostics --example export_replay -- $Db $Session $Name
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    cargo test -q -p aim-diagnostics --test replay_baselines -- --ignored record_missing_baselines --nocapture
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host ""
    Write-Host "Read the transcript before committing it; it is every byte the vehicle sent." -ForegroundColor Yellow
    Write-Host "  core\diagnostics\tests\replays\$Name.transcript"
    Write-Host "  core\diagnostics\tests\replays\$Name.baseline.json"
}
finally {
    Pop-Location
}
