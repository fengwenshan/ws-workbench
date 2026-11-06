# Collect ws-workbench VPN diagnosis logs into one zip.
#
# Run on the affected Windows machine (Administrator recommended):
#   powershell -ExecutionPolicy Bypass -File .\collect-vpn-logs.ps1
#
# The script copies worker.log / script.log / script.prev.log / powershell.log
# plus the generated vpnc script, redacts request.json, and snapshots the
# network state. Nothing is uploaded; the zip stays in the current folder.
param(
    # 默认写到“运行脚本时的当前目录”，不再写到桌面。
    # 需要指定目录时显式传 -OutPath。
    [string]$OutPath,
    # Optional extra root(s) to scan. Useful for the dev/portable build, whose
    # session lives under target\debug\ws-workbench-dev instead of APPDATA.
    [string[]]$Root
)

if ([string]::IsNullOrWhiteSpace($OutPath)) {
    $OutPath = (Get-Location).Path
}

$ErrorActionPreference = 'Continue'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$work = Join-Path $env:TEMP ('ws-vpn-logs-' + [guid]::NewGuid().ToString('N'))
$zip = Join-Path $OutPath ('ws-workbench-vpn-logs-' + $stamp + '.zip')
New-Item -ItemType Directory -Force -Path $work | Out-Null

function Write-Note([string]$name, [string]$text) {
    $path = Join-Path $work $name
    $text | Out-File -LiteralPath $path -Encoding utf8
}

Write-Note 'README.txt' @'
ws-workbench VPN diagnosis bundle
================================

Contents
- sessions\session-N\worker.log        main worker log (steps, errors, heartbeats)
- sessions\session-N\script.log        vpnc-script transcript (UTF-16LE, commands, output, exit codes)
- sessions\session-N\script.prev.log   previous run transcript, if any
- sessions\session-N\powershell.log    every PowerShell command + full output
- sessions\session-N\vpnc-script-win.js generated JScript actually passed to openconnect
- sessions\session-N\request.redacted.json  connection request, password removed
- sessions\session-N\elevate.* / *.cmd  worker launch and scheduled-task registration logs
- system\*.txt                         ipconfig / route / adapters / NRPT / DNS / VPN task snapshots

Notes
- request.json is never copied as-is: the password field is replaced by ***.
- Some system sections require Administrator; if they say "Access denied",
  rerun this script from an elevated PowerShell window.
- worker.log records whether the worker was elevated and which exe it ran.
'@

# ---------------------------------------------------------------- sessions
$roots = @(
    (Join-Path $env:APPDATA 'com.ws.workbench'),
    (Join-Path $env:LOCALAPPDATA 'com.ws.workbench')
)
if ($Root) { $roots += $Root }
$sessions = @()
foreach ($root in $roots) {
    if (Test-Path -LiteralPath $root) {
        $sessions += Get-ChildItem -LiteralPath $root -Recurse -Filter 'worker.log' -File -ErrorAction SilentlyContinue |
            ForEach-Object { $_.Directory.FullName }
    }
}
$sessions = @($sessions | Sort-Object -Unique)
Write-Note 'sessions.txt' ($(if ($sessions.Count -eq 0) { 'No worker.log found under APPDATA/LOCALAPPDATA\com.ws.workbench' } else { $sessions -join "`r`n" }))

$index = 0
foreach ($session in $sessions) {
    $index++
    $dst = Join-Path $work ('sessions\session-' + $index)
    New-Item -ItemType Directory -Force -Path $dst | Out-Null

    foreach ($name in @('worker.log', 'script.log', 'script.prev.log', 'powershell.log', 'status.json', 'main.alive', 'elevate.exit', 'elevate.out', 'create-task.cmd', 'run-worker.cmd', 'delete-task.cmd')) {
        $src = Join-Path $session $name
        if (Test-Path -LiteralPath $src) {
            Copy-Item -LiteralPath $src -Destination (Join-Path $dst $name) -Force
        }
    }

    $generated = Join-Path $session 'script\vpnc-script-win.js'
    if (Test-Path -LiteralPath $generated) {
        Copy-Item -LiteralPath $generated -Destination (Join-Path $dst 'vpnc-script-win.js') -Force
    }

    # request.json contains the plaintext password; redact before copying.
    $requestPath = Join-Path $session 'request.json'
    if (Test-Path -LiteralPath $requestPath) {
        try {
            # request.json 是 Rust 写的 UTF-8（无 BOM）；PS 5.1 默认按 ANSI 读，
            # 中文 profile/username 会变成乱码，必须显式指定 UTF8。
            $request = Get-Content -LiteralPath $requestPath -Raw -Encoding UTF8 | ConvertFrom-Json
            if ($null -ne $request.PSObject.Properties['password']) {
                $request.password = '***REDACTED***'
            }
            $request | ConvertTo-Json -Depth 20 | Out-File -LiteralPath (Join-Path $dst 'request.redacted.json') -Encoding utf8
        } catch {
            "Could not parse request.json; it was NOT copied. Error: $($_.Exception.Message)" |
                Out-File -LiteralPath (Join-Path $dst 'request.redacted.json') -Encoding utf8
        }
    }
}

# ---------------------------------------------------------------- system
$sys = Join-Path $work 'system'
New-Item -ItemType Directory -Force -Path $sys | Out-Null

function Save-Command([string]$name, [scriptblock]$block) {
    $path = Join-Path $sys ($name + '.txt')
    try {
        & $block 2>&1 | Out-String -Width 240 | Out-File -LiteralPath $path -Encoding utf8
    } catch {
        "ERROR: $($_.Exception.Message)" | Out-File -LiteralPath $path -Encoding utf8
    }
}

& cmd.exe /c 'ipconfig /all' 2>&1 | Out-File -LiteralPath (Join-Path $sys 'ipconfig.txt') -Encoding utf8
& cmd.exe /c 'route print -4' 2>&1 | Out-File -LiteralPath (Join-Path $sys 'routes-ipv4.txt') -Encoding utf8
& cmd.exe /c 'route print -6' 2>&1 | Out-File -LiteralPath (Join-Path $sys 'routes-ipv6.txt') -Encoding utf8

Save-Command 'adapters' { Get-NetAdapter | Format-List Name,InterfaceDescription,Status,LinkSpeed,ifIndex }
Save-Command 'ipconfig-cmdlets' { Get-NetIPConfiguration | Format-List InterfaceAlias,InterfaceIndex,IPv4Address,IPv4DefaultGateway,DNSServer }
Save-Command 'routes' { Get-NetRoute -AddressFamily IPv4 | Format-Table DestinationPrefix,NextHop,InterfaceIndex,RouteMetric,PolicyStore -AutoSize }
Save-Command 'nrpt' { Get-DnsClientNrptRule | Format-List DisplayName,Namespace,NameServers,Comment }
Save-Command 'dns-servers' { Get-DnsClientServerAddress | Format-Table InterfaceIndex,InterfaceAlias,AddressFamily,ServerAddresses -AutoSize }
Save-Command 'dnscache' { Get-Service -Name Dnscache | Format-List Status,StartType }
Save-Command 'openconnect-process' { Get-Process -Name openconnect -ErrorAction SilentlyContinue | Format-List Id,Path,StartTime }
Save-Command 'ws-workbench-process' { Get-Process -Name 'ws-workbench' -ErrorAction SilentlyContinue | Format-List Id,Path,StartTime }
Save-Command 'processes' { Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Select-Object ProcessId,Name,ExecutablePath | Sort-Object Name | Format-Table -AutoSize }
Save-Command 'vpn-task' { schtasks.exe /Query /TN WsWorkbenchVpnWorker /V /FO LIST }
Save-Command 'vpn-task-xml' { schtasks.exe /Query /TN WsWorkbenchVpnWorker /XML }
Save-Command 'os' { Get-CimInstance Win32_OperatingSystem | Format-List Caption,Version,BuildNumber,OSArchitecture }
Save-Command 'windows-version' { Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' | Format-List ProductName,DisplayVersion,CurrentBuild,UBR }

# ---------------------------------------------------------------- zip
if (-not (Test-Path -LiteralPath $OutPath)) {
    New-Item -ItemType Directory -Force -Path $OutPath | Out-Null
}
Compress-Archive -Path (Join-Path $work '*') -DestinationPath $zip -Force
Remove-Item -LiteralPath $work -Recurse -Force

Write-Host ''
Write-Host ('Collected: ' + $zip)
Write-Host ('Sessions:  ' + $sessions.Count)
if ($sessions.Count -eq 0) {
    Write-Host 'No ws-workbench VPN session log was found. Did the app run on this machine?'
}
