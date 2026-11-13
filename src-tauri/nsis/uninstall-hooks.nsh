; ws-workbench 的 NSIS 卸载钩子（Windows）。
;
; 目标与 macOS 的「卸载 ws-workbench.command」对齐：
;   1. 卸载前先确认 ws-workbench 进程全部退出；用户同意才强杀，杀不掉就中止，
;      绝不出现「程序删了、进程还在」的半完成状态。
;   2. 读 App 写下的真实数据/附件路径（uninstall-info.txt），弹出三类多选框：
;      设置/数据库/剪贴板历史、附件目录、缓存/日志/偏好设置；默认全不勾（保留），
;      只删除用户勾选的类别。
;   3. 自定义附件目录只提示、不删除，避免误删共享目录。
;
; 静默卸载（/S）不做任何交互，按「保留数据」处理。
;
; TODO(platform-verify:windows): 勾选界面与删除结果需在真实 Windows 上验收；
; 验证步骤：安装后运行 uninstaller，分别测试「全不勾」「只勾数据」「只勾缓存」
; 「勾自定义附件」「取消勾选弹窗」「/S 静默卸载」；期望：全不勾保留所有数据，
; 只删除勾选类别，自定义附件目录不被删除，静默卸载不删数据。
;
; 这个文件由 tauri.conf.json 的 bundle.windows.nsis.installerHooks 引入，
; 必须是带 BOM 的 UTF-8（NSIS 3 才会按 Unicode 解析中文）。

!include "LogicLib.nsh"
!include "FileFunc.nsh"

Var /GLOBAL wsWbRunning
Var /GLOBAL wsWbInfo
Var /GLOBAL wsWbInfoFile
Var /GLOBAL wsWbCustom
Var /GLOBAL wsWbTries
Var /GLOBAL wsWbDelData
Var /GLOBAL wsWbDelUpload
Var /GLOBAL wsWbDelCache

; 检查 ws-workbench.exe（含提权 worker，同一个 exe）是否还在运行。
; 用 find 的退出码判断，避开 tasklist 本地化后的 "INFO: No tasks..." 文案。
Function un.wsWbCheckRunning
  nsExec::ExecToStack 'cmd /c tasklist /FI "IMAGENAME eq ws-workbench.exe" /NH | find /I "ws-workbench.exe"'
  Pop $0
  Pop $1
  ${If} $0 == "0"
    StrCpy $wsWbRunning "1"
  ${Else}
    StrCpy $wsWbRunning "0"
  ${EndIf}
FunctionEnd

; 读 App 写下的路径清单，只取 data_dir / upload_dir / cache_dir 三类行。
Function un.wsWbReadInfo
  StrCpy $wsWbInfo ""
  StrCpy $wsWbCustom "0"
  StrCpy $wsWbInfoFile "$APPDATA\com.ws.workbench\uninstall-info.txt"
  ${IfNot} ${FileExists} "$wsWbInfoFile"
    StrCpy $wsWbInfoFile "$APPDATA\ws-workbench\uninstall-info.txt"
  ${EndIf}
  ${If} ${FileExists} "$wsWbInfoFile"
    FileOpen $0 "$wsWbInfoFile" r
    ${Do}
      FileRead $0 $1
      ${If} $1 == ""
        ${ExitDo}
      ${EndIf}
      ; 去掉行尾 CR/LF（FileRead 会把换行一起读进来）
      ${Do}
        StrCpy $2 $1 1 -1
        ${If} $2 == "$\r"
        ${OrIf} $2 == "$\n"
          StrCpy $1 $1 -1
        ${Else}
          ${ExitDo}
        ${EndIf}
      ${Loop}
      StrCpy $2 $1 14
      ${If} $2 == "upload_custom="
        StrCpy $3 $1 1 14
        ${If} $3 == "1"
          StrCpy $wsWbCustom "1"
        ${EndIf}
      ${Else}
        StrCpy $2 $1 9
        ${If} $2 == "data_dir="
          StrCpy $wsWbInfo "$wsWbInfo$\r$\n$1"
        ${Else}
          StrCpy $2 $1 11
          ${If} $2 == "upload_dir="
            StrCpy $wsWbInfo "$wsWbInfo$\r$\n$1"
          ${Else}
            StrCpy $2 $1 10
            ${If} $2 == "cache_dir="
              StrCpy $wsWbInfo "$wsWbInfo$\r$\n$1"
            ${EndIf}
          ${EndIf}
        ${EndIf}
      ${EndIf}
    ${Loop}
    FileClose $0
  ${EndIf}
FunctionEnd

; 写一行卸载日志，落在数据目录外，删数据后仍可查。
Function un.wsWbLog
  Exch $1
  FileOpen $0 "$APPDATA\ws-workbench-uninstall.log" a
  ${If} $0 != ""
    FileWrite $0 "$1$\r$\n"
    FileClose $0
  ${EndIf}
  Pop $1
FunctionEnd

; 数据勾选界面：Windows 用 PowerShell WinForms 实现多选，对齐 macOS 的 choose from list。
; 脚本由 App 写在数据目录里（uninstall-choose-data.ps1），会读取 uninstall-info.txt
; 展示真实路径；用户勾选结果写到 %APPDATA%\ws-wb-uninstall-selection.txt。
; 静默卸载 / 脚本缺失 / 调用失败 / 用户取消：一律按「保留全部数据」处理。
Function un.wsWbChooseData
  StrCpy $wsWbDelData "0"
  StrCpy $wsWbDelUpload "0"
  StrCpy $wsWbDelCache "0"
  ${If} ${Silent}
    Return
  ${EndIf}

  StrCpy $0 "$APPDATA\com.ws.workbench\uninstall-choose-data.ps1"
  ${IfNot} ${FileExists} "$0"
    StrCpy $0 "$APPDATA\ws-workbench\uninstall-choose-data.ps1"
  ${EndIf}
  ${IfNot} ${FileExists} "$0"
    Push "未找到数据勾选脚本，按保留数据处理"
    Call un.wsWbLog
    Return
  ${EndIf}

  Delete "$APPDATA\ws-wb-uninstall-selection.txt"
  nsExec::ExecToStack 'powershell -NoProfile -STA -ExecutionPolicy Bypass -WindowStyle Hidden -File "$0"'
  Pop $1
  Pop $2

  ${IfNot} ${FileExists} "$APPDATA\ws-wb-uninstall-selection.txt"
    Push "未获得数据勾选结果，按保留数据处理"
    Call un.wsWbLog
    Return
  ${EndIf}

  ClearErrors
  FileOpen $3 "$APPDATA\ws-wb-uninstall-selection.txt" r
  IfErrors ws_wb_choose_done

  ${Do}
    FileRead $3 $4
    ${If} $4 == ""
      ${ExitDo}
    ${EndIf}
    ; 去掉行尾 CR/LF（FileRead 会把换行一起读进来）
    ${Do}
      StrCpy $5 $4 1 -1
      ${If} $5 == "$\r"
      ${OrIf} $5 == "$\n"
        StrCpy $4 $4 -1
      ${Else}
        ${ExitDo}
      ${EndIf}
    ${Loop}
    StrCpy $5 $4 5
    ${If} $5 == "data="
      StrCpy $6 $4 1 5
      ${If} $6 == "1"
        StrCpy $wsWbDelData "1"
      ${EndIf}
    ${EndIf}
    StrCpy $5 $4 7
    ${If} $5 == "upload="
      StrCpy $6 $4 1 7
      ${If} $6 == "1"
        StrCpy $wsWbDelUpload "1"
      ${EndIf}
    ${EndIf}
    StrCpy $5 $4 6
    ${If} $5 == "cache="
      StrCpy $6 $4 1 6
      ${If} $6 == "1"
        StrCpy $wsWbDelCache "1"
      ${EndIf}
    ${EndIf}
  ${Loop}
  FileClose $3
  Delete "$APPDATA\ws-wb-uninstall-selection.txt"

  ws_wb_choose_done:
FunctionEnd

!macro NSIS_HOOK_PREUNINSTALL
  ; 数据目录按当前用户定位；安装器固定 currentUser，这里再兜一次。
  SetShellVarContext current
  ; 静默卸载不弹窗、不杀进程、不删数据：只做最小化的文件移除。
  IfSilent ws_wb_silent

  ; 1) 进程检查
  Call un.wsWbCheckRunning
  ${If} $wsWbRunning == "1"
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "ws-workbench 正在运行，继续卸载需要先退出它。$\r$\n$\r$\n强制退出可能导致正在编辑的数据丢失，是否继续？" IDYES ws_wb_kill IDNO ws_wb_abort

    ws_wb_kill:
      Push "检测到 ws-workbench 正在运行，尝试强制退出"
      Call un.wsWbLog
      nsExec::ExecToStack 'taskkill /IM ws-workbench.exe /F /T'
      Pop $0
      Pop $1
      StrCpy $wsWbTries "0"
      ${Do}
        Sleep 500
        Call un.wsWbCheckRunning
        ${If} $wsWbRunning == "0"
          ${ExitDo}
        ${EndIf}
        IntOp $wsWbTries $wsWbTries + 1
        ${If} $wsWbTries >= 20
          ${ExitDo}
        ${EndIf}
      ${Loop}
      ${If} $wsWbRunning == "1"
        Push "强制退出失败：仍有 ws-workbench 进程，卸载中止且未删除任何文件"
        Call un.wsWbLog
        MessageBox MB_OK|MB_ICONSTOP "卸载失败：仍有 ws-workbench 进程未退出，未删除任何文件。$\r$\n请手动退出应用（或结束任务）后重试。"
        Abort
      ${EndIf}
      Push "ws-workbench 已退出，继续卸载"
      Call un.wsWbLog
      Goto ws_wb_continue

    ws_wb_abort:
      Push "用户取消：应用仍在运行，卸载中止"
      Call un.wsWbLog
      Abort
  ${EndIf}

  ws_wb_continue:

  ; 2) 勾选要清除的数据类别（默认全部保留）；静默卸载/脚本缺失都按保留处理。
  Call un.wsWbReadInfo
  Call un.wsWbChooseData

  ${If} $wsWbDelData == "1"
    Push "用户选择删除数据目录（设置、数据库与剪贴板历史）"
    Call un.wsWbLog
    RMDir /r "$APPDATA\com.ws.workbench"
    RMDir /r "$APPDATA\ws-workbench"
    RMDir /r "$PROFILE\.ws-workbench"
  ${EndIf}
  ${If} $wsWbDelCache == "1"
    Push "用户选择删除缓存、日志与偏好设置"
    Call un.wsWbLog
    RMDir /r "$LOCALAPPDATA\com.ws.workbench"
    RMDir /r "$LOCALAPPDATA\ws-workbench"
  ${EndIf}
  ${If} $wsWbDelUpload == "1"
  ${AndIf} $wsWbCustom == "1"
    Push "自定义附件目录不自动删除，需用户按提示自行处理"
    Call un.wsWbLog
  ${EndIf}
  ${If} $wsWbDelData != "1"
  ${AndIf} $wsWbDelCache != "1"
  ${AndIf} $wsWbDelUpload != "1"
    Push "用户选择保留全部用户数据"
    Call un.wsWbLog
  ${EndIf}

  ws_wb_silent:
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  SetShellVarContext current
  ; 只清理由本程序创建、且已经为空的目录；用户数据目录若保留则不动。
  RMDir "$APPDATA\com.ws.workbench"
  RMDir "$APPDATA\ws-workbench"
  Push "卸载完成"
  Call un.wsWbLog
!macroend
