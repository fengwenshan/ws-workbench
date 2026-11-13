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

$ErrorActionPreference = 'SilentlyContinue'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$info = Join-Path $env:APPDATA 'com.ws.workbench\uninstall-info.txt'
if (-not (Test-Path -LiteralPath $info)) {
    $info = Join-Path $env:APPDATA 'ws-workbench\uninstall-info.txt'
}
$upload = ''
$custom = '0'
if (Test-Path -LiteralPath $info) {
    foreach ($line in (Get-Content -LiteralPath $info -Encoding UTF8)) {
        if ($line.StartsWith('upload_dir=')) { $upload = $line.Substring(11) }
        elseif ($line.StartsWith('upload_custom=')) { $custom = $line.Substring(14) }
    }
}

$form = New-Object System.Windows.Forms.Form
$form.Text = 'ws-workbench 卸载 - 选择要清除的数据'
$form.ClientSize = New-Object System.Drawing.Size(440, 304)
$form.StartPosition = 'CenterScreen'
$form.FormBorderStyle = 'FixedDialog'
$form.MaximizeBox = $false
$form.MinimizeBox = $false
$form.TopMost = $true

$label = New-Object System.Windows.Forms.Label
$label.Text = '默认全部保留。勾选需要删除的数据类别（删除后不可恢复）：'
$label.Location = New-Object System.Drawing.Point(16, 14)
$label.Size = New-Object System.Drawing.Size(408, 20)
$form.Controls.Add($label)

$list = New-Object System.Windows.Forms.CheckedListBox
$list.Location = New-Object System.Drawing.Point(16, 40)
$list.Size = New-Object System.Drawing.Size(408, 150)
$list.CheckOnClick = $true
$list.Items.Add('设置、数据库与剪贴板历史') | Out-Null
$list.Items.Add('附件目录') | Out-Null
$list.Items.Add('缓存、日志与偏好设置') | Out-Null
$form.Controls.Add($list)

$hintText = "数据目录：$env:APPDATA\com.ws.workbench"
if ($upload -ne '') { $hintText = "$hintText`r`n附件目录：$upload" }
if ($custom -eq '1') { $hintText = "$hintText（自定义目录，卸载程序不会自动删除）" }
$hint = New-Object System.Windows.Forms.Label
$hint.Text = $hintText
$hint.Location = New-Object System.Drawing.Point(16, 196)
$hint.Size = New-Object System.Drawing.Size(408, 40)
$form.Controls.Add($hint)

$ok = New-Object System.Windows.Forms.Button
$ok.Text = '下一步'
$ok.Location = New-Object System.Drawing.Point(234, 254)
$ok.Size = New-Object System.Drawing.Size(92, 28)
$ok.DialogResult = [System.Windows.Forms.DialogResult]::OK
$form.Controls.Add($ok)

$cancel = New-Object System.Windows.Forms.Button
$cancel.Text = '取消'
$cancel.Location = New-Object System.Drawing.Point(332, 254)
$cancel.Size = New-Object System.Drawing.Size(92, 28)
$cancel.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
$form.Controls.Add($cancel)

$form.AcceptButton = $ok
$form.CancelButton = $cancel

$result = $form.ShowDialog()
$target = Join-Path $env:APPDATA 'ws-wb-uninstall-selection.txt'
if ($result -ne [System.Windows.Forms.DialogResult]::OK) {
    'cancel=1' | Set-Content -LiteralPath $target -Encoding ASCII
    exit 0
}

$data = 0
$uploadDel = 0
$cache = 0
if ($list.GetItemChecked(0)) { $data = 1 }
if ($list.GetItemChecked(1)) { $uploadDel = 1 }
if ($list.GetItemChecked(2)) { $cache = 1 }
@("data=$data", "upload=$uploadDel", "cache=$cache") |
    Set-Content -LiteralPath $target -Encoding ASCII
exit 0
