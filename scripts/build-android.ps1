<#
.SYNOPSIS
    Build the Android APK on Windows, with or without Developer Mode.

.DESCRIPTION
    `npx tauri android build` compiles the Rust library and then symlinks it
    into the Gradle project. Windows only lets an ordinary user create
    symlinks in Developer Mode, so without it the build compiles everything
    and then fails on that one step.

    This script runs the Tauri build, and when the only thing that failed was
    the symlink, copies the library into place and runs Gradle itself. With
    Developer Mode on, the Tauri build simply succeeds and the fallback never
    runs. CI builds on Linux and never needs any of this.

.PARAMETER Target
    Rust target: aarch64 (phones), armv7 (old phones), x86_64 (the emulator).

.PARAMETER Release
    Build the release APK instead of the debug one. Signed when
    src-tauri/gen/android/keystore.properties exists; see docs/ANDROID.md.

.EXAMPLE
    scripts/build-android.ps1                  # debug APK for a phone
    scripts/build-android.ps1 -Target x86_64   # debug APK for the emulator
#>
param(
    [ValidateSet('aarch64', 'armv7', 'x86_64')]
    [string]$Target = 'aarch64',
    [switch]$Release
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$app = Join-Path $root 'apps/desktop'
$android = Join-Path $app 'src-tauri/gen/android'

# The SDK and NDK where docs/ANDROID.md installs them, unless already set.
if (-not $env:ANDROID_HOME) { $env:ANDROID_HOME = 'C:\Android\sdk' }
if (-not $env:NDK_HOME) {
    $ndk = Get-ChildItem (Join-Path $env:ANDROID_HOME 'ndk') -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name | Select-Object -Last 1
    if (-not $ndk) { throw "No NDK under $env:ANDROID_HOME\ndk. See docs/ANDROID.md." }
    $env:NDK_HOME = $ndk.FullName
}

$abi = @{ aarch64 = 'arm64-v8a'; armv7 = 'armeabi-v7a'; x86_64 = 'x86_64' }[$Target]
$arch = @{ aarch64 = 'arm64'; armv7 = 'arm'; x86_64 = 'x86_64' }[$Target]
$triple = @{ aarch64 = 'aarch64-linux-android'; armv7 = 'armv7-linux-androideabi'; x86_64 = 'x86_64-linux-android' }[$Target]
$flavor = if ($Release) { 'release' } else { 'debug' }
$flavorTask = (Get-Culture).TextInfo.ToTitleCase($flavor)
$archTask = (Get-Culture).TextInfo.ToTitleCase($arch)

$tauriArgs = @('tauri', 'android', 'build', '--apk', '--target', $Target, '--ci')
if (-not $Release) { $tauriArgs += '--debug' }

# Tauri reports progress on stderr, which Windows PowerShell turns into
# errors under 'Stop'. The exit code is what decides.
Push-Location $app
$ErrorActionPreference = 'Continue'
try {
    $log = & npx @tauriArgs 2>&1 | ForEach-Object { "$_" } | Tee-Object -Variable lines | Out-String
    $ok = $LASTEXITCODE -eq 0
} finally {
    $ErrorActionPreference = 'Stop'
    Pop-Location
}

if (-not $ok) {
    if ($log -notmatch 'symbolic link') {
        throw "The Tauri build failed for a reason other than the symlink; see the output above."
    }
    Write-Host "Symlinks need Developer Mode; copying the library and running Gradle instead."

    $lib = Join-Path $app "src-tauri/target/$triple/$flavor/libai_mechanic_desktop_lib.so"
    $jni = Join-Path $android "app/src/main/jniLibs/$abi"
    New-Item -ItemType Directory -Force $jni | Out-Null
    Copy-Item $lib $jni -Force

    Push-Location $android
    try {
        & ./gradlew.bat "assembleUniversal$flavorTask" "-PtargetList=$Target" "-ParchList=$arch" "-PabiList=$abi" `
            -x "rustBuild$archTask$flavorTask" --console=plain
        if ($LASTEXITCODE -ne 0) { throw "Gradle failed" }
    } finally {
        Pop-Location
    }
}

$apk = Get-ChildItem (Join-Path $android "app/build/outputs/apk/universal/$flavor") -Filter '*.apk' |
    Sort-Object LastWriteTime | Select-Object -Last 1
Write-Host "APK: $($apk.FullName)"
