param(
    [string]$AsioSdk,
    [string]$Libclang,
    [switch]$Release
)

$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'The ASIO probe requires Windows.' }

if (-not $Libclang) { $Libclang = $env:LIBCLANG_PATH }
if (-not $Libclang) {
    $candidates = @('C:\Program Files\LLVM\bin', 'C:\msys64\mingw64\bin')
    $Libclang = $candidates | Where-Object { Test-Path (Join-Path $_ 'libclang.dll') } | Select-Object -First 1
}
if (-not $Libclang -or -not (Test-Path (Join-Path $Libclang 'libclang.dll'))) {
    throw 'Supply -Libclang with the directory containing libclang.dll. This script does not install LLVM.'
}

$keys = @('TEMP', 'TMP', 'CPAL_ASIO_DIR', 'LIBCLANG_PATH', 'BINDGEN_EXTRA_CLANG_ARGS')
$previous = @{}
foreach ($key in $keys) { $previous[$key] = [Environment]::GetEnvironmentVariable($key, 'Process') }

try {
    # Isolate the dependency's SDK download from unrelated temporary directories.
    $buildTemp = Join-Path $env:LOCALAPPDATA 'Temp\opencode\asio-probe-build'
    New-Item -ItemType Directory -Path $buildTemp -Force | Out-Null
    $env:TEMP = $buildTemp
    $env:TMP = $buildTemp
    $env:LIBCLANG_PATH = $Libclang
    # MinGW libclang must generate the same C++ ABI as Rust/MSVC, not GNU layout.
    $env:BINDGEN_EXTRA_CLANG_ARGS = '--target=x86_64-pc-windows-msvc'
    if ($AsioSdk) { $env:CPAL_ASIO_DIR = (Resolve-Path $AsioSdk).Path }
    elseif (-not $env:CPAL_ASIO_DIR -and (Test-Path (Join-Path $buildTemp 'asio_sdk'))) {
        $env:CPAL_ASIO_DIR = Join-Path $buildTemp 'asio_sdk'
    }
    $manifest = Join-Path (Split-Path $PSScriptRoot -Parent) 'Cargo.toml'
    $arguments = @('build', '--locked', '--manifest-path', $manifest, '--features', 'asio-probe', '--bin', 'asio-probe')
    if ($Release) { $arguments += '--release' }
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw "ASIO build failed (exit $LASTEXITCODE)." }
}
finally {
    foreach ($key in $keys) { [Environment]::SetEnvironmentVariable($key, $previous[$key], 'Process') }
}
