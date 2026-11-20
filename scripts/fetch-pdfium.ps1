#Requires -Version 7.0
<#
.SYNOPSIS
  Downloads the pinned PDFium build and copies pdfium.dll next to the given directories.

.DESCRIPTION
  PDF import (B-06) needs PDFium at run time. The binary is not committed (see .gitignore);
  this script fetches a pinned release of bblanchon/pdfium-binaries and verifies its SHA-256
  before extracting, so every machine and CI run gets the same library.

.EXAMPLE
  pwsh scripts/fetch-pdfium.ps1 -Dest target/debug,target/release
  pwsh scripts/fetch-pdfium.ps1 -Arch arm64 -Dest dist/LocalNotebook-arm64
#>
param(
    [ValidateSet('x64', 'arm64')][string]$Arch = 'x64',
    [Parameter(Mandatory)][string[]]$Dest
)
$ErrorActionPreference = 'Stop'
# `pwsh -File x.ps1 -Dest a,b` delivers one string "a,b"; accept both forms.
$Dest = @($Dest | ForEach-Object { $_ -split ',' } | Where-Object { $_ })

# Pinned release. To upgrade: change Tag, recompute both hashes, run the PDF smoke test.
$Tag = 'chromium/8165'
$Hashes = @{
    'x64'   = 'CA7338AE7779B8B1B82F4580305C4D762D214F8DF1D54B47647FE3CC75989DED'
    'arm64' = 'D65E405C59CFFE6FE36C0935B90D40DDF3CE75EF284F3168A28DCF00D0388131'
}
$asset = "pdfium-win-$Arch.tgz"
$url = "https://github.com/bblanchon/pdfium-binaries/releases/download/$Tag/$asset"

$work = Join-Path ([IO.Path]::GetTempPath()) ("lna-pdfium-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
    $archive = Join-Path $work $asset
    Write-Host "Downloading $url"
    Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
    $actual = (Get-FileHash $archive -Algorithm SHA256).Hash
    if ($actual -ne $Hashes[$Arch]) {
        throw "SHA-256 mismatch for $asset`n  expected $($Hashes[$Arch])`n  actual   $actual"
    }
    $extract = Join-Path $work 'x'
    New-Item -ItemType Directory -Path $extract | Out-Null
    tar -xzf $archive -C $extract
    if ($LASTEXITCODE -ne 0) { throw 'tar extraction failed' }
    $dll = Join-Path $extract 'bin/pdfium.dll'
    $license = Join-Path $extract 'LICENSE'
    foreach ($d in $Dest) {
        New-Item -ItemType Directory -Force -Path $d | Out-Null
        Copy-Item $dll $d -Force
        Copy-Item $license (Join-Path $d 'PDFium-LICENSE.txt') -Force
        Write-Host "pdfium.dll ($Tag, $Arch) -> $d"
    }
}
finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
