<#
.SYNOPSIS
    Checks and installs the Windows prerequisites for building AI Mechanic.

.DESCRIPTION
    The garage laptop starts with none of this. This script reports what is
    present, installs what is missing, and tells you plainly when something
    needs a reboot or a manual step it should not do on your behalf.

    What it checks, in dependency order:

      1. Visual Studio C++ Build Tools  - the MSVC linker Rust needs
      2. Rust (MSVC toolchain)          - cargo, rustc, clippy, rustfmt
      3. WebView2 runtime               - what the Tauri UI renders in
      4. Node.js LTS                    - for the React UI

    Nothing here touches the vehicle, and nothing needs the adapter plugged in.

.PARAMETER CheckOnly
    Report what is missing and change nothing. Run this first.

.PARAMETER SkipNode
    Skip Node.js. The Rust core and the API build and run without it; only the
    desktop UI needs it.

.PARAMETER Yes
    Do not prompt before installing.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\setup-windows.ps1 -CheckOnly

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\setup-windows.ps1
#>

[CmdletBinding()]
param(
    [switch]$CheckOnly,
    [switch]$SkipNode,
    [switch]$Yes
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:Missing   = @()
$script:Installed = @()
$script:Manual    = @()

function Write-Head($text) {
    Write-Host ''
    Write-Host $text -ForegroundColor Cyan
    Write-Host ('-' * $text.Length) -ForegroundColor DarkCyan
}
function Write-Ok($text)   { Write-Host "  [ok]      $text" -ForegroundColor Green }
function Write-Gap($text)  { Write-Host "  [missing] $text" -ForegroundColor Yellow }
function Write-Note($text) { Write-Host "  [note]    $text" -ForegroundColor DarkGray }
function Write-Bad($text)  { Write-Host "  [failed]  $text" -ForegroundColor Red }

function Test-Command($name) {
    $null -ne (Get-Command $name -ErrorAction SilentlyContinue)
}

# An installer that just ran has almost certainly changed the machine and user
# PATH, but this process still holds the environment block it started with. Pull
# both registry values back in so a freshly installed tool is findable now,
# rather than only in the next terminal.
function Update-PathFromRegistry {
    $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $user    = [Environment]::GetEnvironmentVariable('Path', 'User')
    $env:Path = (@($machine, $user) | Where-Object { $_ }) -join ';'
}

function Confirm-Install($what) {
    if ($Yes) { return $true }
    $answer = Read-Host "  Install $what now? [Y/n]"
    return ($answer -eq '' -or $answer -match '^[Yy]')
}

# winget is the least surprising installer on a modern Windows box. Where it is
# absent we fall back to a direct download, and where neither is appropriate we
# say so rather than guessing.
#
# `Verify` names a command that must exist afterwards. A zero exit code from
# winget is not on its own proof that anything was installed - it also covers
# "already installed", a no-op upgrade, and at least one case observed on this
# project where winget reported success and put nothing on disk. Reporting a
# missing prerequisite as installed is worse than failing: it moves the
# discovery to `cargo build`, where the error names something else entirely.
function Install-WithWinget($id, $label, $verify) {
    if (-not (Test-Command 'winget')) {
        Write-Note "winget is unavailable, so $label cannot be installed automatically."
        return $false
    }
    Write-Host "  Installing $label via winget..." -ForegroundColor White
    # --disable-interactivity keeps this usable over a remote session.
    winget install --id $id --exact --accept-source-agreements --accept-package-agreements --disable-interactivity
    $code = $LASTEXITCODE
    if ($code -ne 0) {
        Write-Bad "winget exited $code installing $label"
        return $false
    }
    if (-not $verify) { return $true }

    Update-PathFromRegistry
    if (Test-Command $verify) { return $true }

    Write-Bad "winget reported success but '$verify' is still not on PATH"
    Write-Note "If it needs a reboot, reboot and re-run this script; otherwise install $label by hand."
    return $false
}

Write-Host ''
Write-Host 'AI Mechanic - Windows prerequisites' -ForegroundColor White
Write-Host '===================================' -ForegroundColor White
if ($CheckOnly) { Write-Note 'Check-only run: nothing will be installed.' }

# --------------------------------------------------------------- environment
Write-Head '0. Environment'

$arch = $env:PROCESSOR_ARCHITECTURE
Write-Ok "Windows $([System.Environment]::OSVersion.Version) ($arch)"
if ($arch -ne 'AMD64' -and $arch -ne 'ARM64') {
    Write-Note "Unexpected architecture $arch; the MSVC toolchain expects AMD64 or ARM64."
}
if (Test-Command 'winget') {
    Write-Ok 'winget is available'
} else {
    Write-Gap 'winget is not available - installs will need to be done manually'
    Write-Note 'winget ships with App Installer from the Microsoft Store.'
}

# ------------------------------------------------- 1. VS C++ Build Tools
# Rust's MSVC toolchain links with MSVC. Without the C++ build tools, `cargo
# build` fails at the link step with a "linker `link.exe` not found" error that
# looks like a Rust problem and is not, so this is checked first.
Write-Head '1. Visual Studio C++ Build Tools (the MSVC linker)'

function Test-MsvcBuildTools {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) { return $false }
    $found = & $vswhere -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath 2>$null
    return -not [string]::IsNullOrWhiteSpace($found)
}

if (Test-MsvcBuildTools) {
    Write-Ok 'MSVC C++ build tools are installed'
} else {
    Write-Gap 'MSVC C++ build tools are not installed'
    $script:Missing += 'VS C++ Build Tools'
    if (-not $CheckOnly -and (Confirm-Install 'Visual Studio Build Tools')) {
        # The workload matters: a bare Build Tools install without
        # VCTools+Windows SDK still has no link.exe.
        $ok = $false
        if (Test-Command 'winget') {
            Write-Host '  Installing Visual Studio Build Tools with the C++ workload...' -ForegroundColor White
            winget install --id Microsoft.VisualStudio.2022.BuildTools --exact `
                --accept-source-agreements --accept-package-agreements `
                --override '--quiet --wait --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
            # Same reasoning as Install-WithWinget: confirm the linker actually
            # arrived rather than trusting the exit code.
            $ok = ($LASTEXITCODE -eq 0) -and (Test-MsvcBuildTools)
            if ($LASTEXITCODE -eq 0 -and -not $ok) {
                Write-Bad 'winget reported success but the C++ workload is still not present'
            }
        }
        if ($ok) {
            Write-Ok 'Visual Studio Build Tools installed'
            $script:Installed += 'VS C++ Build Tools'
        } else {
            Write-Bad 'Automatic install did not complete'
            $script:Manual += 'VS C++ Build Tools: https://visualstudio.microsoft.com/visual-cpp-build-tools/ (select "Desktop development with C++")'
        }
    }
}

# ------------------------------------------------------------- 2. Rust
Write-Head '2. Rust (MSVC toolchain)'

if (Test-Command 'cargo') {
    $cargoVersion = (cargo --version) -join ''
    Write-Ok $cargoVersion
    $host_ = (rustc -vV | Select-String '^host:').ToString()
    if ($host_ -match 'msvc') {
        Write-Ok "Toolchain is $($host_ -replace 'host:\s*','')"
    } else {
        Write-Gap "Toolchain is $($host_ -replace 'host:\s*','') - this project expects the MSVC toolchain"
        Write-Note 'Fix with:  rustup default stable-x86_64-pc-windows-msvc'
        $script:Manual += 'Switch Rust to the MSVC toolchain'
    }
    # This workspace pins rust-version = 1.82 and uses edition 2021.
    if ($cargoVersion -match 'cargo (\d+)\.(\d+)') {
        $minor = [int]$Matches[2]
        if ($minor -lt 82) {
            Write-Gap "cargo 1.$minor is older than the 1.82 this workspace requires"
            Write-Note 'Fix with:  rustup update stable'
            $script:Manual += 'Update Rust to 1.82 or newer'
        }
    }
} else {
    Write-Gap 'Rust is not installed'
    $script:Missing += 'Rust'
    if (-not $CheckOnly -and (Confirm-Install 'Rust (rustup)')) {
        if (Install-WithWinget 'Rustlang.Rustup' 'Rust' 'cargo') {
            Write-Ok 'Rust installed'
            $script:Installed += 'Rust'
            Write-Note 'Open a new terminal so PATH picks up cargo.'
        } else {
            # rustup-init is small and reliable; use it when winget is absent.
            try {
                $installer = Join-Path $env:TEMP 'rustup-init.exe'
                $url = if ($arch -eq 'ARM64') {
                    'https://static.rust-lang.org/rustup/dist/aarch64-pc-windows-msvc/rustup-init.exe'
                } else {
                    'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe'
                }
                Write-Host "  Downloading rustup-init from $url ..." -ForegroundColor White
                Invoke-WebRequest -Uri $url -OutFile $installer -UseBasicParsing
                & $installer -y --default-toolchain stable --profile default
                Write-Ok 'Rust installed'
                $script:Installed += 'Rust'
                Write-Note 'Open a new terminal so PATH picks up cargo.'
            } catch {
                Write-Bad "rustup install failed: $_"
                $script:Manual += 'Rust: https://rustup.rs'
            }
        }
    }
}

# ---------------------------------------------------------- 3. WebView2
# Only the Tauri UI needs this. Windows 11 and most patched Windows 10 machines
# already have it.
Write-Head '3. WebView2 runtime (for the desktop UI)'

function Test-WebView2 {
    $keys = @(
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
        'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
        'HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    )
    foreach ($k in $keys) {
        if (Test-Path $k) {
            $v = (Get-ItemProperty $k -ErrorAction SilentlyContinue).pv
            if ($v -and $v -ne '0.0.0.0') { return $v }
        }
    }
    return $null
}

$webview = Test-WebView2
if ($webview) {
    Write-Ok "WebView2 runtime $webview"
} else {
    Write-Gap 'WebView2 runtime not detected'
    $script:Missing += 'WebView2'
    if (-not $CheckOnly -and (Confirm-Install 'the WebView2 runtime')) {
        if (Install-WithWinget 'Microsoft.EdgeWebView2Runtime' 'WebView2') {
            Write-Ok 'WebView2 runtime installed'
            $script:Installed += 'WebView2'
        } else {
            $script:Manual += 'WebView2: https://developer.microsoft.com/microsoft-edge/webview2/'
        }
    }
}

# -------------------------------------------------------------- 4. Node
Write-Head '4. Node.js (for the React UI)'

if ($SkipNode) {
    Write-Note 'Skipped. The Rust core and the API do not need Node.'
} elseif (Test-Command 'node') {
    $nodeVersion = (node --version) -join ''
    Write-Ok "Node $nodeVersion"
    if ($nodeVersion -match 'v(\d+)\.') {
        $major = [int]$Matches[1]
        if ($major -lt 18) {
            Write-Gap "Node $nodeVersion is older than the v18 Tauri 2 tooling expects"
            $script:Manual += 'Update Node to an LTS release (v20 or v22)'
        }
    }
    if (Test-Command 'npm') { Write-Ok "npm $((npm --version) -join '')" }
} else {
    Write-Gap 'Node.js is not installed'
    $script:Missing += 'Node.js'
    if (-not $CheckOnly -and (Confirm-Install 'Node.js LTS')) {
        if (Install-WithWinget 'OpenJS.NodeJS.LTS' 'Node.js LTS' 'node') {
            Write-Ok 'Node.js installed'
            $script:Installed += 'Node.js'
            Write-Note 'Open a new terminal so PATH picks up node and npm.'
        } else {
            $script:Manual += 'Node.js LTS: https://nodejs.org/'
        }
    }
}

# ------------------------------------------------------- 5. Bluetooth adapter
# Informational only. Pairing is a user action and this script will not attempt
# to change the machine's Bluetooth state.
Write-Head '5. ELM327 adapter (informational)'

try {
    $bt = Get-PnpDevice -Class Bluetooth -ErrorAction SilentlyContinue |
          Where-Object { $_.Status -eq 'OK' }
    if ($bt) { Write-Ok "Bluetooth radio present ($($bt.Count) device(s) OK)" }
    else     { Write-Note 'No active Bluetooth radio detected.' }
} catch {
    Write-Note 'Could not query Bluetooth devices.'
}

try {
    $ports = [System.IO.Ports.SerialPort]::GetPortNames()
    if ($ports) {
        Write-Ok "Serial ports: $($ports -join ', ')"
        Write-Note 'A paired ELM327 appears here as an OUTGOING COM port.'
        Write-Note 'Confirm which one it is with: GET /api/v1/adapters/ports?probe=true'
    } else {
        Write-Note 'No COM ports yet. Pair the ELM327 first:'
        Write-Note '  Settings > Bluetooth & devices > Add device > the ELM327 (PIN is usually 1234 or 0000)'
        Write-Note '  then Bluetooth settings > More Bluetooth options > COM Ports, and use the Outgoing port.'
    }
} catch {
    Write-Note 'Could not enumerate COM ports.'
}

# ------------------------------------------------------------------ summary
Write-Head 'Summary'

if ($script:Installed.Count -gt 0) {
    Write-Host "  Installed: $($script:Installed -join ', ')" -ForegroundColor Green
}
if ($script:Manual.Count -gt 0) {
    Write-Host '  Needs a manual step:' -ForegroundColor Yellow
    foreach ($m in $script:Manual) { Write-Host "    - $m" -ForegroundColor Yellow }
}
if ($CheckOnly -and $script:Missing.Count -gt 0) {
    Write-Host "  Missing: $($script:Missing -join ', ')" -ForegroundColor Yellow
    Write-Host '  Run this script again without -CheckOnly to install them.' -ForegroundColor Yellow
}
if ($script:Missing.Count -eq 0 -and $script:Manual.Count -eq 0) {
    Write-Host '  Everything needed is present.' -ForegroundColor Green
}

Write-Host ''
Write-Host 'Next steps' -ForegroundColor White
Write-Host '----------' -ForegroundColor DarkGray
Write-Host '  Open a NEW terminal (so PATH is refreshed), then from the repo root:'
Write-Host ''
Write-Host '    cargo build --workspace' -ForegroundColor White
Write-Host '    cargo test  --workspace' -ForegroundColor White
Write-Host ''
Write-Host '  Run the API against the virtual truck - no adapter needed:'
Write-Host ''
Write-Host '    cargo run -p aim-api -- --simulator --scenario dpf-regen' -ForegroundColor White
Write-Host ''
Write-Host '  Then against the real adapter, once it is paired:'
Write-Host ''
Write-Host '    cargo run -p aim-api -- --serial COM5' -ForegroundColor White
Write-Host ''

if ($script:Installed.Count -gt 0) {
    Write-Note 'Some installs need a new terminal, and the Build Tools may want a reboot.'
}
