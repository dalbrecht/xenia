$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$env:CARGO_TARGET_DIR = Join-Path (Get-Location).Path 'target'
# Cargo's encoded flags preserve paths containing spaces and existing custom flags.
$flags = if ($env:CARGO_ENCODED_RUSTFLAGS) { @($env:CARGO_ENCODED_RUSTFLAGS -split [char]31) } elseif ($env:RUSTFLAGS) { @($env:RUSTFLAGS -split '\s+' | Where-Object { $_ }) } else { @() }
if ($env:USERPROFILE) { $flags += "--remap-path-prefix=$env:USERPROFILE=/xenia-build/home" }
if ($env:CARGO_HOME) { $flags += "--remap-path-prefix=$env:CARGO_HOME=/xenia-build/cargo" }
$flags += "--remap-path-prefix=$((Get-Location).Path)=/xenia-build/source"
$env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]31
cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
dotnet publish apps/windows-worker/XeniaWorker.csproj -c Release -r win-x64 --self-contained true -p:DebugType=None -p:DebugSymbols=false "-p:PathMap=$((Get-Location).Path)=/_/src" -o dist/windows-worker
if ($LASTEXITCODE -ne 0) { throw '.NET publish failed' }
Copy-Item target/release/xenia.exe dist/windows-worker/xenia-engine.exe -Force
Copy-Item LICENSE dist/windows-worker/LICENSE -Force
Write-Host 'Built dist/windows-worker/XeniaWorker.exe'
