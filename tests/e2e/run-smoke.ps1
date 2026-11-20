# Runs the end-to-end smoke test: mock LLM + lna-devtools serve + Node WebSocket client.
# Usage (repo root):  pwsh tests/e2e/run-smoke.ps1 [-Pdf]
param([switch]$Pdf)
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$root = Resolve-Path "$here/../.."
$bin  = Join-Path $root 'target/debug/lna-devtools.exe'
$work = Join-Path ([IO.Path]::GetTempPath()) ("lna-e2e-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $work | Out-Null
if (-not (Test-Path $bin)) { throw "Build first: cargo build -p lna-devtools" }

$mock  = Start-Process python -ArgumentList '-I', "$here/mock_llm.py", '18555', "$work/llm.log" -PassThru -NoNewWindow
$serve = Start-Process $bin -ArgumentList '--data-dir', "$work/data", 'serve', '--addr', '127.0.0.1:18493' `
         -RedirectStandardOutput "$work/serve.log" -RedirectStandardError "$work/serve.err" -PassThru -NoNewWindow
try {
    $url = $null
    for ($i = 0; $i -lt 30 -and -not $url; $i++) {
        Start-Sleep -Milliseconds 500
        if (Test-Path "$work/serve.log") {
            $m = Select-String -Path "$work/serve.log" -Pattern 'ws://\S+/ws\?token=[0-9a-f-]+' | Select-Object -First 1
            if ($m) { $url = $m.Matches[0].Value }
        }
    }
    if (-not $url) { throw "Dev server did not start; see $work/serve.err" }
    node "$here/smoke.mjs" $url 18555
    if ($Pdf) {
        python -I "$here/make_pdf.py" "$work/t.pdf"
        node "$here/pdf.mjs" $url "$work/t.pdf"
    }
} finally {
    Stop-Process -Id $serve.Id, $mock.Id -ErrorAction SilentlyContinue
    Write-Host "Logs: $work"
}
