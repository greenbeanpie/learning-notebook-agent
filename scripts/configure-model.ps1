#Requires -Version 7.3
# 为测试客户端配置模型（客户端“模型协议配置”按钮尚未实现）。
# 用法（在本目录运行）：
#   pwsh .\configure-model.ps1 -Protocol chat_completion -BaseUrl https://api.example.com/v1 -Model your-model
# Protocol 可选：chat_completion | responses | anthropic | gemini
# 运行后按提示输入 API Key（不回显，保存到 Windows 凭据管理器）。本地模型无需 Key 时直接回车。
# 也可预先设置环境变量 LNA_API_KEY，脚本将直接使用而不再提示。
param(
    [Parameter(Mandatory)][ValidateSet('chat_completion', 'responses', 'anthropic', 'gemini')][string]$Protocol,
    [Parameter(Mandatory)][string]$BaseUrl,
    [Parameter(Mandatory)][string]$Model,
    [string]$Name = "$Protocol / $Model",
    [int]$ContextWindow = 32000,
    [int]$MaxOutputTokens = 8000
)
$ErrorActionPreference = 'Stop'
$cli = Join-Path $PSScriptRoot 'lna-devtools.exe'
$dataDir = Join-Path $env:LOCALAPPDATA 'LocalNotebook'

function Invoke-Core([string]$method, [hashtable]$params) {
    $json = $params | ConvertTo-Json -Compress -Depth 5
    $out = & $cli --data-dir $dataDir call $method $json 2>$null
    $line = @($out | Where-Object { $_ -like '{*' })[-1]
    if (-not $line) { throw "$method 无输出" }
    $resp = $line | ConvertFrom-Json
    if ($resp.error) { throw "$method 失败：$($resp.error.code) $($resp.error.message)" }
    return $resp.result
}

$prof = Invoke-Core 'profile.upsert' @{
    display_name = $Name; protocol = $Protocol; base_url = $BaseUrl; model_id = $Model
    context_window = $ContextWindow; max_output_tokens = $MaxOutputTokens; activate = $true
}
$id = $prof.id
Write-Host "已创建并激活配置 $id"

if ($env:LNA_API_KEY) {
    $key = $env:LNA_API_KEY
} else {
    $secure = Read-Host -AsSecureString 'API Key（无需 Key 直接回车）'
    $key = [Runtime.InteropServices.Marshal]::PtrToStringUni(
        [Runtime.InteropServices.Marshal]::SecureStringToGlobalAllocUnicode($secure))
}
if ($key) {
    Invoke-Core 'secret.set' @{ keychain_ref = "profile_$id"; secret = $key } | Out-Null
    $key = $null
    $exists = (Invoke-Core 'secret.exists' @{ keychain_ref = "profile_$id" }).exists
    if (-not $exists) { throw 'API Key 未能写入 Windows 凭据管理器' }
    Write-Host 'API Key 已保存到 Windows 凭据管理器'
}

Invoke-Core 'profile.activate' @{ profile_id = $id } | Out-Null
Write-Host '正在测试连接…'
try {
    $t = Invoke-Core 'profile.test' @{ profile_id = $id }
    Write-Host "连接成功，模型回复：$($t.reply)"
} catch {
    Write-Warning $_
}
Write-Host '完成。客户端里重新发送消息即可使用（必要时重启客户端）。'
