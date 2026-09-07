#!/usr/bin/env pwsh
# build-engine.ps1 — Build the Rust engine binary (Windows / macOS / Linux).
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = Resolve-Path "$PSScriptRoot/.."
Push-Location $root
try {
    Write-Host "==> Building Rust engine (release) ..."
    cargo build --release --manifest-path "$root/engine/Cargo.toml"
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    $bin = if ($IsWindows) {
        "engine/target/release/tv-obsbroadcast-scheduler.exe"
    } else {
        "engine/target/release/tv-obsbroadcast-scheduler"
    }
    if (-not (Test-Path $bin)) {
        throw "engine binary not produced at $bin"
    }
    Write-Host "==> Built engine: $bin"
} finally {
    Pop-Location
}
