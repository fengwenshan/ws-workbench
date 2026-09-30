# 重启开发环境：结束旧的 ws-tools / vite / tauri dev 进程，再重新拉起一份。
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File scripts\dev-restart.ps1
#
# 说明：
#   - 本机 PATH 里的 pnpm 来自 TRAE 自带的 corepack 0.32.0，它给 pnpm 12 生成的 shim 指向
#     bin/pnpm.cjs，而 pnpm 12 只提供 bin/pnpm.mjs，调用必然 MODULE_NOT_FOUND；
#     另装一份能跑通的 pnpm 12.5.1 又会把自身写进 pnpm-lock.yaml（CI 用的 pnpm 10 读不懂）。
#     所以这里绕开 pnpm：直接用 node_modules 里的 tauri CLI，并用 -c 把 beforeDevCommand 覆盖成 npm run dev。
#   - 只结束属于本仓库的进程（按命令行里的仓库路径匹配），不误伤其他项目
#   - 检测到 openconnect 仍在运行时直接退出：先断开 VPN 再重启，否则会留下无人回收的路由
#   - 本文件必须保存为带 BOM 的 UTF-8。Windows PowerShell 5.1 对无 BOM 的 .ps1 按 ANSI 解码，
#     中文注释会被解成乱码并把脚本解析坏。

[CmdletBinding()]
param(
    [int]$WaitSeconds = 3
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# ---------- 1. 定位 tauri CLI ----------
$tauri = Join-Path $root 'node_modules\.bin\tauri.cmd'
if (-not (Test-Path $tauri)) {
    Write-Warning "找不到 $tauri，先安装依赖再重启"
    exit 1
}

# tauri.conf.json 里 beforeDevCommand 是 `pnpm dev`，本机 pnpm 不可用，覆盖成 npm。
# 不能用 Set-Content -Encoding UTF8：PS 5.1 会写入 BOM，tauri 的 JSON 解析器不接受开头的 BOM。
$override = Join-Path $env:TEMP 'ws-tools-dev-override.json'
[System.IO.File]::WriteAllText($override, '{"build":{"beforeDevCommand":"npm run dev"}}')

# ---------- 2. 前置检查 ----------
if (Get-Process openconnect -ErrorAction SilentlyContinue) {
    Write-Warning 'openconnect 正在运行：先断开 VPN，再执行重启'
    exit 1
}

# ---------- 3. 结束旧进程 ----------
$escapedRoot = [regex]::Escape($root)
$stale = New-Object System.Collections.Generic.List[int]

Get-Process ws-tools -ErrorAction SilentlyContinue | ForEach-Object { $stale.Add($_.Id) }

Get-CimInstance Win32_Process -Filter "Name = 'node.exe' OR Name = 'cargo.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -and $_.CommandLine -match $escapedRoot -and $_.CommandLine -match 'vite|tauri' } |
    ForEach-Object { $stale.Add([int]$_.ProcessId) }

Get-NetTCPConnection -LocalPort 8790 -State Listen -ErrorAction SilentlyContinue |
    Where-Object { $_.OwningProcess } |
    ForEach-Object { $stale.Add([int]$_.OwningProcess) }

$targets = $stale | Sort-Object -Unique | Where-Object { $_ -and $_ -ne $PID }
if ($targets.Count -gt 0) {
    Write-Host "结束旧进程: $($targets -join ', ')"
    foreach ($procId in $targets) {
        Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Seconds $WaitSeconds
} else {
    Write-Host '没有需要结束的旧进程'
}

# ---------- 4. 启动 ----------
Write-Host '启动 tauri dev（beforeDevCommand 覆盖为 npm run dev）'
& $tauri dev -c $override
