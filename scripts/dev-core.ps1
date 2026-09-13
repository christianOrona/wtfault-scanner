# Run the diagnostic core straight from source against a real adapter.
#
# WHY THIS EXISTS
#
# Testing a change to the core by running `npm run tauri:build` compiles the
# whole workspace in release mode and bundles two installers: about six minutes
# before anything can be tried. Most of what is being tested is driven over HTTP
# and never touches the desktop shell at all.
#
# This runs the same core as a plain binary. An incremental debug build after a
# small change is tens of seconds rather than minutes, and `--profiles` points
# at the working tree, so editing a vehicle profile needs no compile whatever.
#
# Measured on 2026-09-12, with a truck connected and three separate fixes to
# make: six minutes a cycle became well under one.
#
# USAGE
#
#   scripts\dev-core.ps1                 # simulator, port 8788
#   scripts\dev-core.ps1 -Serial COM4    # a real adapter
#
# The desktop app must not be running: it holds both the serial port and the
# database. Stop it first.
param(
    [string]$Serial,
    [int]$Port = 8788
)

$repo = Split-Path -Parent $PSScriptRoot
$db = Join-Path $env:APPDATA "ai-mechanic\data\sessions.sqlite"
$profiles = Join-Path $repo "vehicle-profiles"
# The same providers.json the desktop app uses. Without this the core looks
# somewhere else, finds no model configured, and the whole assistant appears
# missing — which cost a six-minute release build to work around once.
$settings = Join-Path $env:APPDATA "ai-mechanic\data\providers.json"

# The same database the installed app uses, so an imported as-built file, a
# discovered module list and a recorded session are all still there. Testing
# against an empty database is testing a different vehicle.
$args = @("run", "-p", "aim-api", "--", "--port", $Port, "--db", $db, "--profiles", $profiles, "--settings", $settings)
if ($Serial) { $args += @("--serial", $Serial) } else { $args += "--simulator" }

Write-Host "core on http://127.0.0.1:$Port  db=$db" -ForegroundColor Cyan
if ($Serial) { Write-Host "adapter: $Serial" -ForegroundColor Cyan } else { Write-Host "adapter: simulator" -ForegroundColor Cyan }

Set-Location $repo
& cargo @args
