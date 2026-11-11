#requires -Version 5.1
<#
.SYNOPSIS
    Windows 上实测「屏幕休眠 / 保持唤醒」是否真的生效：屏幕是否保持常亮、系统是否不进入空闲睡眠。

.DESCRIPTION
    判据全部来自操作系统，不依赖人眼看屏幕：

    * 屏幕开关状态：RegisterPowerSettingNotification(GUID_CONSOLE_DISPLAY_STATE)，
      收到 0 = 关屏、1 = 亮、2 = 变暗。
    * 系统是否睡过：GetTickCount64()（含睡眠时间）与 QueryUnbiasedInterruptTime()（不含睡眠）
      两个时钟的差值。
    * 用户是否在动鼠标/键盘：GetLastInputInfo()。有输入会重置空闲计时，这类相位判为「无效」。

    四个相位（默认每个 100 秒）：

      P0 对照（无请求）：AC 关屏超时 = -DisplayTimeoutSeconds 秒 → 期望屏幕到点真的关掉。
                         这一相位如果关不掉，说明这台机器的显示电源路径不可观测
                         （无头/虚拟显示），或会话一直有输入注入，后续结论一律不可信，
                         脚本以退出码 2 结束。
      P1 保持唤醒（屏幕常亮）：持有 ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED，
                         同样的关屏超时 → 期望屏幕全程不关。
      P2 保持唤醒（允许屏幕关闭）：持有 ES_CONTINUOUS | ES_SYSTEM_REQUIRED，
                         关屏超时 -DisplayTimeoutSeconds 秒 + 睡眠超时 -SleepTimeoutSeconds 秒
                         → 期望屏幕关掉、但系统不睡（同时验证「允许屏幕关闭」只放开关屏）。
      P3 对照组（可选，-IncludeSleepControl）：不持有任何请求，睡眠超时 -SleepTimeoutSeconds 秒
                         → 期望系统真的睡着。跑完需要人工按一下键盘把机器唤醒。
                         这个相位会真的让机器睡眠，默认不跑。

    所有被临时改动的电源设置都在 finally 里还原（包括脚本异常中断）。

.PARAMETER SecondsPerPhase
    每个相位的等待秒数，默认 100（必须明显大于关屏/睡眠超时）。

.PARAMETER DisplayTimeoutSeconds
    相位里临时使用的 AC 关屏超时（秒），默认 60。会话空闲时间达不到 60 秒的机器
    （有人操作、或有输入注入）可以调小到 15–30 秒，否则对照组永远等不到关屏。

.PARAMETER SleepTimeoutSeconds
    相位里临时使用的 AC 睡眠超时（秒），默认 60。

.PARAMETER IncludeSleepControl
    额外跑 P3：验证「没有保持唤醒时机器确实会睡」。会让机器真的睡过去。

.PARAMETER LogFile
    日志路径，默认 <仓库>\.tmp\verify-keepawake-<时间戳>.log。

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-keepawake-windows.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-keepawake-windows.ps1 -SecondsPerPhase 110 -IncludeSleepControl

.EXAMPLE
    # 会话不太空闲的机器：把关屏超时压到 20 秒
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-keepawake-windows.ps1 -DisplayTimeoutSeconds 20 -SecondsPerPhase 60

.NOTES
    需要管理员权限：要临时改 AC 的「关闭屏幕 / 睡眠」超时（powercfg /setacvalueindex）。
    跑完请确认 AC 超时已恢复原值（脚本会打印还原前后的值）。
#>
[CmdletBinding()]
param(
    [int]$SecondsPerPhase = 100,
    [int]$DisplayTimeoutSeconds = 60,
    [int]$SleepTimeoutSeconds = 60,
    [switch]$IncludeSleepControl,
    [string]$LogFile = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

# ----------------------------------------------------------------- 基础设施

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $LogFile) {
    $logDir = Join-Path $repoRoot '.tmp'
    if (-not (Test-Path -LiteralPath $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
    $LogFile = Join-Path $logDir ('verify-keepawake-{0}.log' -f (Get-Date).ToString('yyyyMMdd-HHmmss'))
}

function Write-Log {
    param([string]$Text)
    $line = '[{0}] {1}' -f (Get-Date).ToString('HH:mm:ss'), $Text
    Add-Content -LiteralPath $LogFile -Value $line -Encoding UTF8
    Write-Host $line
}

function Assert-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# 电源设置 GUID（Windows 公开常量）
$SUB_VIDEO = '7516b95f-f776-4464-8c53-06167f40cc99'
$SET_VIDEO_IDLE = '3c0bc021-c8a8-4e07-a973-6b14cbcb2b7e'
$SUB_SLEEP = '238c9fa8-0aad-41ed-83f4-97be242c8f20'
$SET_STANDBY_IDLE = '29f6c1db-86da-48c5-9fdb-f2b67b1f44da'

function Get-ActiveSchemeGuid {
    $text = (powercfg /getactivescheme 2>&1 | Out-String)
    if ($text -match '([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})') {
        return $Matches[1].ToLowerInvariant()
    }
    throw "无法解析当前电源方案 GUID，powercfg 输出：$text"
}

function Get-PowerIndexSeconds {
    param([string]$SubGroup, [string]$Setting, [ValidateSet('AC', 'DC')][string]$Line = 'AC')
    $scheme = Get-ActiveSchemeGuid
    $path = "HKLM:\SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes\$scheme\$SubGroup\$Setting"
    $value = (Get-ItemProperty -LiteralPath $path -ErrorAction Stop)."$($Line)SettingIndex"
    return [int64]$value
}

function Set-AcTimeoutSeconds {
    param([ValidateSet('monitor', 'standby')][string]$Kind, [int]$Seconds)
    $sub = if ($Kind -eq 'monitor') { $SUB_VIDEO } else { $SUB_SLEEP }
    $setting = if ($Kind -eq 'monitor') { $SET_VIDEO_IDLE } else { $SET_STANDBY_IDLE }
    $null = powercfg /setacvalueindex SCHEME_CURRENT $sub $setting $Seconds
    if ($LASTEXITCODE -ne 0) { throw "powercfg /setacvalueindex $Kind=$Seconds 失败（退出码 $LASTEXITCODE）" }
    $null = powercfg /setactive SCHEME_CURRENT
    if ($LASTEXITCODE -ne 0) { throw "powercfg /setactive 失败（退出码 $LASTEXITCODE）" }
}

# ----------------------------------------------------------------- 探测代码

$probeSource = @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;

public class ProbeResult
{
    public List<string> Events = new List<string>();
    public double SleepSeconds;
    public double IdleSeconds;
    public string ThreadStateNote = "";
    public bool SawOff;
    public double FirstOffSeconds = -1;
    public double MaxIdleSeconds;

    public string EventsText()
    {
        if (Events.Count == 0) return "(none)";
        return string.Join(", ", Events.ToArray());
    }
}

public static class KeepAwakeProbe
{
    private const uint WM_POWERBROADCAST = 0x0218;
    private const uint PBT_POWERSETTINGCHANGE = 0x8015;
    private const int GWLP_WNDPROC = -4;
    private const uint WM_SYSCOMMAND = 0x0112;
    private const int SC_MONITORPOWER = 0xF170;
    private const uint SMTO_ABORTIFHUNG = 0x0002;

    private static readonly IntPtr HWND_BROADCAST = new IntPtr(0xFFFF);
    private static readonly Guid ConsoleDisplayState = new Guid("6FE69556-704A-47A0-8F24-C28D936FDA47");

    [StructLayout(LayoutKind.Sequential)]
    private struct POWERBROADCAST_SETTING
    {
        public Guid PowerSetting;
        public uint DataLength;
        public byte Data;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct LASTINPUTINFO
    {
        public uint cbSize;
        public uint dwTime;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MSG
    {
        public IntPtr hwnd;
        public uint message;
        public IntPtr wParam;
        public IntPtr lParam;
        public uint time;
        public int x;
        public int y;
    }

    private delegate IntPtr WndProcDelegate(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern IntPtr CreateWindowExW(uint exStyle, string cls, string name, uint style,
        int x, int y, int width, int height, IntPtr parent, IntPtr menu, IntPtr instance, IntPtr param);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr SetWindowLongPtrW(IntPtr hWnd, int index, IntPtr value);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr CallWindowProcW(IntPtr proc, IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr RegisterPowerSettingNotification(IntPtr recipient, ref Guid guid, uint flags);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool UnregisterPowerSettingNotification(IntPtr handle);

    [DllImport("user32.dll")]
    private static extern bool PeekMessageW(out MSG msg, IntPtr hWnd, uint filterMin, uint filterMax, uint remove);

    [DllImport("user32.dll")]
    private static extern bool TranslateMessage(ref MSG msg);

    [DllImport("user32.dll")]
    private static extern IntPtr DispatchMessageW(ref MSG msg);

    [DllImport("user32.dll")]
    private static extern IntPtr SendMessageTimeoutW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam,
        uint flags, uint timeout, out IntPtr result);

    [DllImport("user32.dll")]
    private static extern bool SetCursorPos(int x, int y);

    [DllImport("user32.dll")]
    private static extern bool GetLastInputInfo(ref LASTINPUTINFO info);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint SetThreadExecutionState(uint flags);

    [DllImport("kernel32.dll")]
    private static extern ulong GetTickCount64();

    [DllImport("kernel32.dll")]
    private static extern bool QueryUnbiasedInterruptTime(out ulong unbiasedTime);

    private static WndProcDelegate procDelegate;
    private static IntPtr procOriginal;
    private static ProbeResult current;
    private static ulong startTick;

    private static double Elapsed()
    {
        return (GetTickCount64() - startTick) / 1000.0;
    }

    private static IntPtr WndProc(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam)
    {
        if (msg == WM_POWERBROADCAST && (uint)wParam == PBT_POWERSETTINGCHANGE && lParam != IntPtr.Zero)
        {
            var setting = Marshal.PtrToStructure<POWERBROADCAST_SETTING>(lParam);
            if (setting.PowerSetting == ConsoleDisplayState && setting.DataLength >= 4)
            {
                int state = Marshal.ReadInt32(lParam, Marshal.SizeOf(typeof(Guid)) + 4);
                string name = state == 0 ? "OFF" : state == 1 ? "on" : state == 2 ? "dimmed" : ("state" + state);
                double at = Elapsed();
                lock (current.Events)
                {
                    current.Events.Add(string.Format("{0}@{1:F1}s", name, at));
                    if (state == 0)
                    {
                        current.SawOff = true;
                        if (current.FirstOffSeconds < 0) current.FirstOffSeconds = at;
                    }
                }
            }
        }
        return CallWindowProcW(procOriginal, hWnd, msg, wParam, lParam);
    }

    private static void BroadcastMonitorPower(int what)
    {
        IntPtr result;
        SendMessageTimeoutW(HWND_BROADCAST, WM_SYSCOMMAND, new IntPtr(SC_MONITORPOWER), new IntPtr(what),
            SMTO_ABORTIFHUNG, 5000, out result);
    }

    private static void NudgeInput()
    {
        SetCursorPos(10, 10);
        SetCursorPos(20, 20);
    }

    private static uint CurrentIdleSeconds()
    {
        var info = new LASTINPUTINFO();
        info.cbSize = (uint)Marshal.SizeOf(typeof(LASTINPUTINFO));
        if (!GetLastInputInfo(ref info)) return 0;
        int idleMs = Environment.TickCount - (int)info.dwTime;
        if (idleMs < 0) idleMs = 0;
        return (uint)(idleMs / 1000);
    }

    /// <summary>
    /// 跑一个相位。mode: none | system | display。
    ///   system  = ES_CONTINUOUS | ES_SYSTEM_REQUIRED                          （允许屏幕关闭）
    ///   display = ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED    （屏幕保持常亮）
    /// 返回该相位的观测结果。
    /// </summary>
    public static ProbeResult Run(int seconds, string mode)
    {
        var result = new ProbeResult();
        current = result;

        IntPtr hwnd = CreateWindowExW(0, "STATIC", "keepawake-probe", 0, 0, 0, 0, 0,
            new IntPtr(-3) /* HWND_MESSAGE */, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
        if (hwnd == IntPtr.Zero) throw new InvalidOperationException("CreateWindowEx 失败: " + Marshal.GetLastWin32Error());

        procDelegate = WndProc;
        procOriginal = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, Marshal.GetFunctionPointerForDelegate(procDelegate));
        Guid guid = ConsoleDisplayState;
        IntPtr registration = RegisterPowerSettingNotification(hwnd, ref guid, 0 /* DEVICE_NOTIFY_WINDOW_HANDLE */);
        if (registration == IntPtr.Zero)
        {
            throw new InvalidOperationException("RegisterPowerSettingNotification 失败: " + Marshal.GetLastWin32Error());
        }

        uint flags = 0;
        if (mode == "system") flags = 0x80000000u | 0x00000002u;
        if (mode == "display") flags = 0x80000000u | 0x00000002u | 0x00000001u;

        ulong unbiasedStart = 0;
        QueryUnbiasedInterruptTime(out unbiasedStart);

        NudgeInput(); // 把空闲计时清零，保证相位内能真正走满一个超时周期
        startTick = GetTickCount64();

        string note = "no execution state set (observer only)";
        if (flags != 0)
        {
            uint previous = SetThreadExecutionState(flags);
            note = string.Format("SetThreadExecutionState(0x{0:X}) -> previous=0x{1:X}", flags, previous);
        }
        result.ThreadStateNote = note;

        ulong deadline = startTick + (ulong)(seconds * 1000);
        uint maxIdle = 0;
        while (GetTickCount64() < deadline)
        {
            MSG msg;
            while (PeekMessageW(out msg, IntPtr.Zero, 0, 0, 1))
            {
                TranslateMessage(ref msg);
                DispatchMessageW(ref msg);
            }
            uint idle = CurrentIdleSeconds();
            if (idle > maxIdle) maxIdle = idle;
            Thread.Sleep(50);
        }

        if (result.SawOff)
        {
            // 把屏幕重新点亮，避免下一个相位从「黑屏」开始
            BroadcastMonitorPower(-1);
            for (int i = 0; i < 30; i++)
            {
                MSG msg;
                while (PeekMessageW(out msg, IntPtr.Zero, 0, 0, 1))
                {
                    TranslateMessage(ref msg);
                    DispatchMessageW(ref msg);
                }
                Thread.Sleep(100);
                if (!result.SawOff) break;
                if (i == 20) { result.Events.Add("force-on@" + Elapsed().ToString("F1") + "s"); result.SawOff = false; }
            }
        }

        if (flags != 0) SetThreadExecutionState(0x80000000u /* ES_CONTINUOUS */);
        if (registration != IntPtr.Zero) UnregisterPowerSettingNotification(registration);

        ulong unbiasedEnd = 0;
        QueryUnbiasedInterruptTime(out unbiasedEnd);
        double biased = (GetTickCount64() - startTick) / 1000.0;
        double unbiased = (unbiasedEnd - unbiasedStart) / 10000000.0;
        result.SleepSeconds = Math.Max(0.0, biased - unbiased);
        result.IdleSeconds = CurrentIdleSeconds();
        result.MaxIdleSeconds = maxIdle;
        return result;
    }
}
'@

Add-Type -TypeDefinition $probeSource -Language CSharp

# ----------------------------------------------------------------- 主流程

if (-not (Assert-Admin)) {
    Write-Host '需要管理员权限：本脚本要临时修改 AC 的「关闭屏幕 / 睡眠」超时。'
    Write-Host '请用“以管理员身份运行”的 PowerShell 重新执行：'
    Write-Host ('  powershell -NoProfile -ExecutionPolicy Bypass -File "{0}"' -f $PSCommandPath)
    exit 3
}

$originalMonitor = Get-PowerIndexSeconds -SubGroup $SUB_VIDEO -Setting $SET_VIDEO_IDLE
$originalStandby = Get-PowerIndexSeconds -SubGroup $SUB_SLEEP -Setting $SET_STANDBY_IDLE

$results = New-Object System.Collections.ArrayList
$verdict = 'UNKNOWN'
$exitCode = 0
$changedSettings = $false

function Invoke-Phase {
    param(
        [string]$Title,
        [ValidateSet('none', 'system', 'display')][string]$Mode,
        [int]$Seconds,
        [int]$StandbySeconds
    )
    Write-Log ''
    Write-Log ('===== {0} =====' -f $Title)
    Write-Log ('mode={0} 等待 {1}s，AC 关屏超时={2}s，AC 睡眠超时={3}s' -f $Mode, $Seconds, $DisplayTimeoutSeconds, $StandbySeconds)

    Set-AcTimeoutSeconds -Kind monitor -Seconds $DisplayTimeoutSeconds
    Set-AcTimeoutSeconds -Kind standby -Seconds $StandbySeconds
    $script:changedSettings = $true

    $monitorNow = Get-PowerIndexSeconds -SubGroup $SUB_VIDEO -Setting $SET_VIDEO_IDLE
    $standbyNow = Get-PowerIndexSeconds -SubGroup $SUB_SLEEP -Setting $SET_STANDBY_IDLE
    if ($monitorNow -ne $DisplayTimeoutSeconds) { throw ('AC 关屏超时没设上，当前 {0} 秒' -f $monitorNow) }
    if ($standbyNow -ne $StandbySeconds) { throw ('AC 睡眠超时没设上，当前 {0} 秒' -f $standbyNow) }

    $result = [KeepAwakeProbe]::Run($Seconds, $Mode)

    Write-Log ('线程状态: {0}' -f $result.ThreadStateNote)
    Write-Log ('显示状态事件: {0}' -f $result.EventsText())
    Write-Log ('相位内最大空闲: {0}s（相位结束时 {1}s）；相位内睡眠时长: {2:F1}s' -f $result.MaxIdleSeconds, $result.IdleSeconds, $result.SleepSeconds)

    $idleOk = ($result.MaxIdleSeconds -ge $DisplayTimeoutSeconds)
    $result | Add-Member -NotePropertyName IdleOk -NotePropertyValue $idleOk -Force
    $row = [pscustomobject]@{
        Phase    = $Title
        Mode     = $Mode
        SawOff   = $result.SawOff
        FirstOff = $result.FirstOffSeconds
        MaxIdle  = $result.MaxIdleSeconds
        Sleep    = [math]::Round($result.SleepSeconds, 1)
        IdleOk   = $idleOk
    }
    [void]$results.Add($row)
    return $result
}

try {
    Write-Log ('日志: {0}' -f $LogFile)
    Write-Log ('原值: AC 关屏={0}s，AC 睡眠={1}s（0 表示从不）' -f $originalMonitor, $originalStandby)
    Write-Log ('OS: {0}' -f ([Environment]::OSVersion.VersionString))
    Write-Log ('每相位 {0}s；临时关屏超时 {1}s、睡眠超时 {2}s；P3 对照 {3}' -f $SecondsPerPhase, $DisplayTimeoutSeconds, $SleepTimeoutSeconds, ($(if ($IncludeSleepControl) { '开启（会让机器真的睡眠）' } else { '关闭' })))

    # P0：对照 —— 没有请求时，关屏超时必须真的把屏幕关掉
    $p0 = Invoke-Phase -Title 'P0 对照（无请求，期望屏幕关闭）' -Mode 'none' -Seconds $SecondsPerPhase -StandbySeconds 0
    if (-not $p0.IdleOk) {
        Write-Log ''
        Write-Log ('P0 期间机器空闲时间从未到过 {0} 秒（最大空闲 {1}s）：关屏超时根本没有机会触发。' -f $DisplayTimeoutSeconds, $p0.MaxIdleSeconds)
        Write-Log '常见原因：有人一直在动鼠标/键盘，或有程序在持续注入输入（远程控制、鼠标抖动器、某些自动化工具）。'
        Write-Log '请把 -DisplayTimeoutSeconds 调小（例如 15）后重跑，或换一台没人碰的机器。'
        Write-Log '这一相位拿不到对照，后面所有“屏幕没关”都不能算证据。'
        $verdict = 'INCONCLUSIVE（会话一直有输入，关屏超时无法触发）'
        $exitCode = 2
        throw '_ABORT_'
    }
    if (-not $p0.SawOff) {
        Write-Log ''
        Write-Log ('P0 空闲已超过 {0} 秒，但没有观测到任何显示状态变化：这台机器的显示电源路径不可观测（无头/虚拟显示，或没有真实显示器）。' -f $DisplayTimeoutSeconds)
        Write-Log '对照缺失，P1/P2 的“屏幕没关”无法证明任何事，脚本终止。'
        $verdict = 'INCONCLUSIVE（无显示状态对照）'
        $exitCode = 2
        throw '_ABORT_'
    }

    # P1：持有显示请求 —— 屏幕必须全程不关
    $p1 = Invoke-Phase -Title 'P1 保持唤醒（屏幕常亮，期望屏幕不关）' -Mode 'display' -Seconds $SecondsPerPhase -StandbySeconds 0

    # P2：只持有系统请求 —— 屏幕该关，系统不该睡
    $p2 = Invoke-Phase -Title 'P2 保持唤醒（允许屏幕关闭，期望关屏但不睡眠）' -Mode 'system' -Seconds $SecondsPerPhase -StandbySeconds $SleepTimeoutSeconds

    $problems = New-Object System.Collections.ArrayList
    if (-not $p1.IdleOk) {
        [void]$problems.Add(('P1 期间机器空闲从未到过 {0}s（最大 {1}s），这段“屏幕没关”不能算证据，请调小 -DisplayTimeoutSeconds 或换机器重跑' -f $DisplayTimeoutSeconds, $p1.MaxIdleSeconds))
    }
    if ($p1.SawOff) { [void]$problems.Add('P1 期间屏幕被关掉了，说明 ES_DISPLAY_REQUIRED 没拦住关屏') }
    if (-not $p2.IdleOk) {
        [void]$problems.Add(('P2 期间机器空闲从未到过 {0}s（最大 {1}s），关屏/睡眠结论无效，请重跑' -f $DisplayTimeoutSeconds, $p2.MaxIdleSeconds))
    } elseif (-not $p2.SawOff) {
        [void]$problems.Add('P2（允许屏幕关闭）空闲已超过关屏超时但屏幕没关：关屏超时没生效，或另有进程持有显示请求')
    }
    if ($p2.Sleep -gt 5) { [void]$problems.Add(('P2 期间系统睡了 {0:F1}s，说明 ES_SYSTEM_REQUIRED 没拦住睡眠' -f $p2.Sleep)) }

    if ($IncludeSleepControl) {
        Write-Log ''
        Write-Log 'P3 会让机器真的睡过去（不持有任何请求），醒来后脚本继续。'
        $p3 = Invoke-Phase -Title 'P3 对照（无请求，期望系统睡眠）' -Mode 'none' -Seconds ($SecondsPerPhase + 30) -StandbySeconds $SleepTimeoutSeconds
        if ($p3.Sleep -le 5) { [void]$problems.Add(('P3 期间系统没睡（{0:F1}s），无法证明“没有保持唤醒时会睡”；可能有别的进程持有系统请求' -f $p3.Sleep)) }
    }

    Write-Log ''
    if ($problems.Count -eq 0) {
        $verdict = 'PASS'
        Write-Log '结论：PASS —— 屏幕常亮请求有效；「允许屏幕关闭」时屏幕按超时关闭、系统仍未睡眠。'
    } else {
        $verdict = 'FAIL'
        $exitCode = 1
        Write-Log '结论：FAIL'
        foreach ($item in $problems) { Write-Log ('  - {0}' -f $item) }
    }
} catch {
    if ($_.Exception.Message -ne '_ABORT_') {
        Write-Log ('中断: {0}' -f $_.Exception.Message)
        if ($verdict -eq 'UNKNOWN') { $verdict = 'ERROR'; $exitCode = 1 }
    }
} finally {
    if ($changedSettings) {
        try {
            Set-AcTimeoutSeconds -Kind monitor -Seconds ([int]$originalMonitor)
            Set-AcTimeoutSeconds -Kind standby -Seconds ([int]$originalStandby)
            $monitorAfter = Get-PowerIndexSeconds -SubGroup $SUB_VIDEO -Setting $SET_VIDEO_IDLE
            $standbyAfter = Get-PowerIndexSeconds -SubGroup $SUB_SLEEP -Setting $SET_STANDBY_IDLE
            Write-Log ('还原: AC 关屏={0}s，AC 睡眠={1}s（原值 {2}s / {3}s）' -f $monitorAfter, $standbyAfter, $originalMonitor, $originalStandby)
            if ($monitorAfter -ne $originalMonitor -or $standbyAfter -ne $originalStandby) {
                Write-Log '警告：电源设置没完全还原，请手动用 powercfg 改回。'
            }
        } catch {
            Write-Log ('警告：还原电源设置失败：{0}' -f $_.Exception.Message)
        }
    }
    # 保险：把屏幕点亮
    try {
        $wake = @'
using System;
using System.Runtime.InteropServices;
public static class WakeUp { 
  [DllImport("user32.dll")] public static extern IntPtr SendMessageTimeoutW(IntPtr h, uint m, IntPtr w, IntPtr l, uint f, uint t, out IntPtr r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  public static void Run() {
    IntPtr r;
    SendMessageTimeoutW(new IntPtr(0xFFFF), 0x0112, new IntPtr(0xF170), new IntPtr(-1), 0x0002, 5000, out r);
    SetCursorPos(10, 10); SetCursorPos(20, 20);
  }
}
'@
        if (-not ('WakeUp' -as [type])) { Add-Type -TypeDefinition $wake -Language CSharp }
        [WakeUp]::Run()
    } catch { }
}

Write-Log ''
if ($results.Count -gt 0) {
    Write-Log '相位汇总：'
    ($results | Format-Table -AutoSize | Out-String).Trim() -split "`r?`n" | ForEach-Object { Write-Log ('  {0}' -f $_) }
}
Write-Log ('结论: {0}' -f $verdict)
Write-Log ('日志文件: {0}' -f $LogFile)
exit $exitCode
