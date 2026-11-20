#Requires -Version 7.0
<#
.SYNOPSIS
  Builds the Rust core and the WinUI client and assembles a runnable folder (default: dist/LocalNotebook-test).

.DESCRIPTION
  Steps: cargo build --release (lna-ffi, lna-devtools) -> dotnet publish (NativeAOT, unpackaged,
  self-contained) -> copy lna_ffi.dll, lna-devtools.exe, pdfium.dll and configure-model.ps1.
  Requires the Visual Studio Installer directory on PATH for NativeAOT (vswhere.exe); it is added here when present.
#>
param(
    [string]$Out = 'dist/LocalNotebook-test'
)
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $root

$vsInstaller = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer'
if (Test-Path $vsInstaller) { $env:Path += ";$vsInstaller" }
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path $cargoBin) { $env:Path += ";$cargoBin" }

cargo build --release -p lna-ffi -p lna-devtools
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

Push-Location apps/windows/LocalNotebook
try {
    dotnet publish -c Release -r win-x64 -p:Platform=x64 -p:WindowsPackageType=None -p:WindowsAppSDKSelfContained=true
    if ($LASTEXITCODE -ne 0) { throw 'dotnet publish failed' }
}
finally { Pop-Location }

$publish = Join-Path $root 'apps/windows/LocalNotebook/bin/x64/Release/net10.0-windows10.0.19041.0/win-x64/publish'
New-Item -ItemType Directory -Force -Path $Out | Out-Null
Copy-Item "$publish/*" $Out -Recurse -Force
Copy-Item 'target/release/lna_ffi.dll' $Out -Force
Copy-Item 'target/release/lna-devtools.exe' $Out -Force
Copy-Item 'scripts/configure-model.ps1' $Out -Force
& (Join-Path $PSScriptRoot 'fetch-pdfium.ps1') -Dest $Out
Write-Host "Client assembled in $Out"
