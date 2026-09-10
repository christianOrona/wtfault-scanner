# Everything that can say "this is broken" without a ten-minute build.
#
#     .\scripts\check.ps1              # the fast loop, about four minutes cold
#     .\scripts\check.ps1 -Quick       # skip the desktop shell, about one
#     .\scripts\check.ps1 -Fix         # format and apply what clippy can fix
#
# Why this exists: the checks were four separate commands, each needing its own
# PATH setup, and the desktop shell was not in any of them. That crate is its
# own cargo workspace -- deliberately, so `cargo test --workspace` stays fast and
# portable for the core -- but the consequence was that nothing routinely
# compiled it except the release build. It holds the database path, the profile
# loader and the in-process API host, so "nothing routinely checks it" was not a
# small gap.
#
# This is also the shape a CI job would take, which is the point: whatever runs
# here should be the thing that runs there.

[CmdletBinding()]
param(
    # Skip the desktop shell. Its dependency tree is large and platform
    # specific, so it dominates a cold run.
    [switch]$Quick,
    # Apply formatting and the clippy suggestions that are machine-applicable.
    [switch]$Fix
)

# Native tools write progress to stderr, and 'Stop' turns that into a
# terminating error even when the tool succeeded - a documented PowerShell 5.1
# trap. Exit codes are checked explicitly below instead.
$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# The tools are frequently not on PATH in a fresh shell, and failing three
# minutes in because of that is a waste of three minutes.
$env:PATH = "$env:USERPROFILE\.cargo\bin;C:\Program Files\nodejs;$env:PATH"

$failures = [System.Collections.Generic.List[string]]::new()
$timings = [System.Collections.Generic.List[object]]::new()

function Invoke-Step {
    param(
        [string]$Name,
        [string]$Directory,
        [string[]]$Command
    )
    Write-Host ""
    Write-Host "-- $Name " -NoNewline -ForegroundColor Cyan
    Write-Host ("-" * [Math]::Max(1, 60 - $Name.Length)) -ForegroundColor DarkGray

    $sw = [Diagnostics.Stopwatch]::StartNew()
    Push-Location (Join-Path $root $Directory)
    try {
        # `2>&1` alone is not enough here: PowerShell 5.1 wraps each stderr line
        # from a native tool in an ErrorRecord and prints it as a
        # NativeCommandError, so cargo's ordinary "Compiling ..." progress looks
        # like a failure. Flattening to strings as they arrive keeps the output
        # readable, and the exit code below is what actually decides.
        & $Command[0] @($Command[1..($Command.Length - 1)]) 2>&1 |
            ForEach-Object { "$_" } |
            Write-Host
        $ok = $LASTEXITCODE -eq 0
    } finally {
        Pop-Location
        $sw.Stop()
    }

    $secs = [math]::Round($sw.Elapsed.TotalSeconds, 1)
    $timings.Add([pscustomobject]@{ Step = $Name; Seconds = $secs; Passed = $ok })
    if (-not $ok) {
        $failures.Add($Name)
        Write-Host "FAILED ($secs s)" -ForegroundColor Red
    } else {
        Write-Host "ok ($secs s)" -ForegroundColor Green
    }
}

if ($Fix) {
    Invoke-Step 'rustfmt (writing)' '.' @('cargo', 'fmt', '--all')
    Invoke-Step 'clippy --fix' '.' @('cargo', 'clippy', '--workspace', '--all-targets', '--fix', '--allow-dirty', '--allow-staged')
}

# Core first: it is the fastest and catches the most, so a broken build is
# reported in seconds rather than after the slow steps have run.
Invoke-Step 'core: tests' '.' @('cargo', 'test', '--workspace', '--locked')
Invoke-Step 'core: clippy' '.' @('cargo', 'clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings')

# The UI's typecheck is part of its build, so this covers both.
Invoke-Step 'ui: typecheck and build' 'apps/desktop' @('npm', 'run', '-s', 'build')

if (-not $Quick) {
    # The build without the serial transport, for machines with no libudev.
    # core/transport/Cargo.toml promises this works; CI checks it, so checking
    # it here too means CI never reports something this script would have
    # caught first.
    Invoke-Step 'core: no serial' '.' @('cargo', 'clippy', '--workspace', '--all-targets', '--no-default-features', '--locked', '--', '-D', 'warnings')

    # The shell, explicitly, because --workspace does not reach it. Must come
    # after the UI build above: the shell embeds `../dist` with `include_dir!`,
    # so it does not compile at all until the UI has been built once.
    Invoke-Step 'shell: clippy' 'apps/desktop/src-tauri' @('cargo', 'clippy', '--all-targets', '--locked', '--', '-D', 'warnings')
}

Write-Host ""
Write-Host ("-" * 62) -ForegroundColor DarkGray
$timings | Format-Table -AutoSize | Out-String | Write-Host

if ($failures.Count -gt 0) {
    Write-Host "$($failures.Count) step(s) failed: $($failures -join ', ')" -ForegroundColor Red
    exit 1
}

$total = ($timings | Measure-Object -Property Seconds -Sum).Sum
Write-Host "everything passed in $([math]::Round($total, 1)) s" -ForegroundColor Green
if ($Quick) {
    Write-Host "note: -Quick skipped the desktop shell. Run without it before building a release." -ForegroundColor Yellow
}
exit 0
