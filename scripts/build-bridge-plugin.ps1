param([switch]$Install)
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'This packaging script targets Windows; the Rust crates can also build on other 64-bit platforms.' }
$root = Split-Path $PSScriptRoot -Parent
& cargo build --locked --release --manifest-path (Join-Path $root 'Cargo.toml') -p metronome-bridge-plugin
if ($LASTEXITCODE -ne 0) { throw 'CLAP plugin build failed.' }
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Path $dist -Force | Out-Null
$plugin = Join-Path $dist 'metronome-dry-audio-bridge.clap'
Copy-Item -LiteralPath (Join-Path $root 'target\release\metronome_bridge_plugin.dll') -Destination $plugin -Force
Write-Output "CLAP plugin: $plugin"
if ($Install) {
    $directory = Join-Path $env:LOCALAPPDATA 'Programs\Common\CLAP'
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    Copy-Item -LiteralPath $plugin -Destination $directory -Force
    Write-Output "Installed to $directory. Rescan/restart REAPER manually. Close plugin instances before replacing a loaded DLL."
}
