#!/usr/bin/env pwsh
# package-plugin.ps1 — Assemble the OBS plugin bundle on Windows.
#
# Required env (set before running):
#   LIBOBS_INCLUDE_DIR   path to <obs-studio>/libobs   (headers)
#   OBS_IMPORT_LIB       path to obs.lib               (import lib generated
#                                                       from an installed
#                                                       obs.dll)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = Resolve-Path "$PSScriptRoot/.."
Push-Location $root
try {
    Write-Host "==> Packaging tv-obsbroadcast-scheduler (Windows x64) ..."

    # 1. Rust engine
    & "$PSScriptRoot/build-engine.ps1"
    if ($LASTEXITCODE -ne 0) { throw "engine build failed" }

    # 2. CMake configure / build (C plugin)
    if (-not $env:LIBOBS_INCLUDE_DIR) {
        throw "LIBOBS_INCLUDE_DIR is required (path to obs-studio/libobs)."
    }
    if (-not $env:OBS_IMPORT_LIB) {
        throw "OBS_IMPORT_LIB is required (path to obs.lib)."
    }
    Push-Location "$root/plugin"
    try {
        $cfg = @("-S", ".", "-B", "build",
                 "-G", "Visual Studio 17 2022", "-A", "x64",
                 "-DLIBOBS_INCLUDE_DIR=$env:LIBOBS_INCLUDE_DIR",
                 "-DOBS_IMPORT_LIB=$env:OBS_IMPORT_LIB")
        & cmake @cfg
        if ($LASTEXITCODE -ne 0) { throw "cmake configure failed" }
        & cmake --build build --config Release
        if ($LASTEXITCODE -ne 0) { throw "cmake build failed" }
    } finally {
        Pop-Location
    }

    # 3. Assemble the bundle that ships to OBS's plugin dir.
    $out = "$root/dist/tv-obsbroadcast-scheduler-windows-x64"
    if (Test-Path $out) { Remove-Item -Recurse -Force $out }
    New-Item -ItemType Directory -Force -Path "$out/engine" | Out-Null
    New-Item -ItemType Directory -Force -Path "$out/data/locale" | Out-Null

    Copy-Item `
        "$root/plugin/build/Release/tv-obsbroadcast-scheduler.dll" `
        $out
    Copy-Item `
        "$root/engine/target/release/tv-obsbroadcast-scheduler.exe" `
        "$out/engine/"
    Copy-Item -Recurse -Force `
        "$root/plugin/data/locale/*" `
        "$out/data/locale/"

    # 4. ZIP
    $zip = "$root/dist/tv-obsbroadcast-scheduler-windows-x64.zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::CreateFromDirectory($out, $zip)
    Write-Host "==> Plugin bundle: $zip"
} finally {
    Pop-Location
}
