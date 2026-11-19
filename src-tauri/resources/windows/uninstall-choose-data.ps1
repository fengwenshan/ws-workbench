# ws-workbench 卸载程序的数据勾选界面（由 NSIS 卸载器调用，Windows 专用）。
#
# 目的：卸载时让用户逐类勾选要清除的数据（默认全部不勾 = 保留），
# 对齐 macOS 卸载脚本的多选交互。路径来自 App 写下的 uninstall-info.txt。
#
# 输出：%APPDATA%\ws-wb-uninstall-selection.txt（ASCII）：
#   data=0|1      设置、数据库与剪贴板历史（%APPDATA%\com.ws.workbench 等）
#   upload=0|1    附件目录（自定义目录只提示，卸载器不会自动删）
#   cache=0|1     缓存、日志与偏好设置（%LOCALAPPDATA%\com.ws.workbench 等）
# 取消 / 关闭窗口：写 cancel=1，卸载器按「保留全部数据」处理。
#
# 界面要求（2026-11 改版）：这是一个独立的 WinForms 窗口，要和主程序一个气质 ——
# 品牌色顶栏、微软雅黑、9pt 正文、圆整的按钮，而不是系统默认的灰底宋体。
# 注意：文件必须带 UTF-8 BOM，Windows PowerShell 5.1 才会按 UTF-8 读中文。

$ErrorActionPreference = 'SilentlyContinue'

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class WsWbDpi {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
'@
[void][WsWbDpi]::SetProcessDPIAware()
[System.Windows.Forms.Application]::EnableVisualStyles()

# —— 读安装时写下的真实路径（自定义附件目录只提示、不删除）——
$infoFile = Join-Path $env:APPDATA 'com.ws.workbench\uninstall-info.txt'
if (-not (Test-Path -LiteralPath $infoFile)) {
    $infoFile = Join-Path $env:APPDATA 'ws-workbench\uninstall-info.txt'
}
$uploadDir = ''
$uploadCustom = $false
if (Test-Path -LiteralPath $infoFile) {
    foreach ($line in (Get-Content -LiteralPath $infoFile -Encoding UTF8)) {
        if ($line.StartsWith('upload_dir=')) {
            $uploadDir = $line.Substring(11).Trim()
        } elseif ($line.StartsWith('upload_custom=')) {
            $uploadCustom = ($line.Substring(14).Trim() -eq '1')
        }
    }
}
if ([string]::IsNullOrWhiteSpace($uploadDir)) {
    $uploadDir = Join-Path $env:USERPROFILE 'Downloads'
}

# —— 与主程序一致的配色（--primary: #3b82f6 / #2563eb）——
$brand = [System.Drawing.Color]::FromArgb(59, 130, 246)
$brandHi = [System.Drawing.Color]::FromArgb(37, 99, 235)
$ink = [System.Drawing.Color]::FromArgb(17, 24, 39)
$muted = [System.Drawing.Color]::FromArgb(107, 114, 128)
$line = [System.Drawing.Color]::FromArgb(229, 231, 235)
$soft = [System.Drawing.Color]::FromArgb(249, 250, 251)
$onBrand = [System.Drawing.Color]::FromArgb(219, 234, 254)

# 高 DPI 下按实际比例换算，既不糊也不挤（96 DPI = 1.0）
$screen = [System.Drawing.Graphics]::FromHwnd([IntPtr]::Zero)
$scale = if ($screen) { $screen.DpiX / 96.0 } else { 1.0 }
if ($screen) { $screen.Dispose() }
function S([double]$value) { return [int][Math]::Round($value * $scale) }

function UiFont([double]$points, [bool]$bold) {
    $style = if ($bold) { [System.Drawing.FontStyle]::Bold } else { [System.Drawing.FontStyle]::Regular }
    foreach ($face in @('Microsoft YaHei UI', 'Microsoft YaHei', 'Segoe UI')) {
        $candidate = New-Object System.Drawing.Font($face, [single]$points, $style, [System.Drawing.GraphicsUnit]::Point)
        if ($candidate.Name -eq $face) { return $candidate }
        $candidate.Dispose()
    }
    return New-Object System.Drawing.Font([System.Drawing.FontFamily]::GenericSansSerif, [single]$points, $style)
}

$form = New-Object System.Windows.Forms.Form
$form.Text = '卸载 ws-workbench'
$form.ClientSize = New-Object System.Drawing.Size((S 520), (S 456))
$form.StartPosition = 'CenterScreen'
$form.FormBorderStyle = 'FixedDialog'
$form.MaximizeBox = $false
$form.MinimizeBox = $false
$form.ShowIcon = $false
$form.TopMost = $true
$form.BackColor = [System.Drawing.Color]::White
$form.Font = UiFont 9 $false

# 品牌色顶栏（渐变，和主程序保持同一套色）
$header = New-Object System.Windows.Forms.Panel
$header.Location = New-Object System.Drawing.Point(0, 0)
$header.Size = New-Object System.Drawing.Size((S 520), (S 88))
$header.Add_Paint({
        param($sender, $paintEvent)
        $rect = New-Object System.Drawing.Rectangle(0, 0, $sender.Width, $sender.Height)
        $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($rect, $brand, $brandHi, 0)
        $paintEvent.Graphics.FillRectangle($brush, $rect)
        $brush.Dispose()
    })
$form.Controls.Add($header)

$title = New-Object System.Windows.Forms.Label
$title.Text = '卸载 ws-workbench'
$title.Font = UiFont 15.5 $true
$title.ForeColor = [System.Drawing.Color]::White
$title.BackColor = [System.Drawing.Color]::Transparent
$title.Location = New-Object System.Drawing.Point((S 24), (S 16))
$title.AutoSize = $true
$header.Controls.Add($title)

$subtitle = New-Object System.Windows.Forms.Label
$subtitle.Text = '先选择要删除的数据，再开始卸载'
$subtitle.Font = UiFont 9.5 $false
$subtitle.ForeColor = $onBrand
$subtitle.BackColor = [System.Drawing.Color]::Transparent
$subtitle.Location = New-Object System.Drawing.Point((S 24), (S 52))
$subtitle.AutoSize = $true
$header.Controls.Add($subtitle)

$intro = New-Object System.Windows.Forms.Label
$intro.Text = '默认保留全部数据。勾选要删除的类别（删除后不可恢复）：'
$intro.Font = UiFont 9 $false
$intro.ForeColor = $ink
$intro.Location = New-Object System.Drawing.Point((S 24), (S 102))
$intro.Size = New-Object System.Drawing.Size((S 472), (S 22))
$form.Controls.Add($intro)

$tooltip = New-Object System.Windows.Forms.ToolTip

# 一行数据类别：面板 + 粗体标题 + 灰色路径说明（路径太长时省略号 + 悬浮提示）
function Add-Row([string]$caption, [string]$detail, [int]$top, [bool]$disabled) {
    $panel = New-Object System.Windows.Forms.Panel
    $panel.Location = New-Object System.Drawing.Point((S 24), $top)
    $panel.Size = New-Object System.Drawing.Size((S 472), (S 72))
    $panel.BorderStyle = 'FixedSingle'
    $panel.BackColor = if ($disabled) { $soft } else { [System.Drawing.Color]::White }

    $check = New-Object System.Windows.Forms.CheckBox
    $check.Text = $caption
    $check.Font = UiFont 9.5 $true
    $check.ForeColor = if ($disabled) { $muted } else { $ink }
    $check.Location = New-Object System.Drawing.Point((S 12), (S 8))
    $check.AutoSize = $true
    $check.Enabled = -not $disabled
    $panel.Controls.Add($check)

    $desc = New-Object System.Windows.Forms.Label
    $desc.Text = $detail
    $desc.Font = UiFont 8.5 $false
    $desc.ForeColor = $muted
    $desc.AutoEllipsis = $true
    $desc.AutoSize = $false
    $desc.Location = New-Object System.Drawing.Point((S 34), (S 36))
    $desc.Size = New-Object System.Drawing.Size((S 424), (S 20))
    $tooltip.SetToolTip($desc, $detail)
    $panel.Controls.Add($desc)

    $form.Controls.Add($panel)
    return $check
}

$rowData = Add-Row '设置、数据库与剪贴板历史' `
    ("配置文件、设备名、剪贴板历史与图片：$env:APPDATA\com.ws.workbench") (S 132) $false

$uploadDetail = if ($uploadCustom) {
    "自定义目录：$uploadDir（需手动删除，卸载程序不会动它）"
} else {
    "接收到的附件：$uploadDir"
}
$rowUpload = Add-Row '附件目录' $uploadDetail (S 214) $uploadCustom

$rowCache = Add-Row '缓存、日志与偏好设置' `
    ("窗口位置、WebView2 缓存与运行日志：$env:LOCALAPPDATA\com.ws.workbench") (S 296) $false

$note = New-Object System.Windows.Forms.Label
$note.Text = '未勾选的类别会原样保留。卸载完成后，程序文件与开始菜单项会被移除。'
$note.Font = UiFont 8 $false
$note.ForeColor = $muted
$note.Location = New-Object System.Drawing.Point((S 24), (S 378))
$note.Size = New-Object System.Drawing.Size((S 472), (S 20))
$form.Controls.Add($note)

function New-Button([string]$text, [int]$x, [int]$width, [bool]$primary) {
    $button = New-Object System.Windows.Forms.Button
    $button.Text = $text
    $button.Font = UiFont 9.5 $false
    $button.FlatStyle = 'Flat'
    $button.Location = New-Object System.Drawing.Point($x, (S 404))
    $button.Size = New-Object System.Drawing.Size($width, (S 34))
    $button.Cursor = [System.Windows.Forms.Cursors]::Hand
    if ($primary) {
        $button.BackColor = $brand
        $button.ForeColor = [System.Drawing.Color]::White
        $button.FlatAppearance.BorderSize = 0
        $button.FlatAppearance.MouseOverBackColor = $brandHi
        $button.FlatAppearance.MouseDownBackColor = $brandHi
    } else {
        $button.BackColor = [System.Drawing.Color]::White
        $button.ForeColor = $ink
        $button.FlatAppearance.BorderSize = 1
        $button.FlatAppearance.BorderColor = $line
        $button.FlatAppearance.MouseOverBackColor = $soft
        $button.FlatAppearance.MouseDownBackColor = $line
    }
    $form.Controls.Add($button)
    return $button
}

$okButton = New-Button '开始卸载' (S 384) (S 112) $true
$okButton.DialogResult = [System.Windows.Forms.DialogResult]::OK
$cancelButton = New-Button '取消' (S 288) (S 88) $false
$cancelButton.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
$form.AcceptButton = $okButton
$form.CancelButton = $cancelButton

$result = $form.ShowDialog()
$target = Join-Path $env:APPDATA 'ws-wb-uninstall-selection.txt'
if ($result -ne [System.Windows.Forms.DialogResult]::OK) {
    'cancel=1' | Set-Content -LiteralPath $target -Encoding ASCII
    exit 0
}

$data = if ($rowData.Checked) { 1 } else { 0 }
$upload = if ($rowUpload.Checked -and -not $uploadCustom) { 1 } else { 0 }
$cache = if ($rowCache.Checked) { 1 } else { 0 }
@("data=$data", "upload=$upload", "cache=$cache") |
    Set-Content -LiteralPath $target -Encoding ASCII
exit 0
