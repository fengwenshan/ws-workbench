; ws-workbench 的 NSIS 卸载钩子（Windows）。
;
; 目标与 macOS 的「卸载 ws-workbench.command」对齐：
;   1. 卸载前先确认 ws-workbench 进程全部退出；用户同意才强杀，杀不掉就中止，
;      绝不出现「程序删了、进程还在」的半完成状态。
;   2. 展示 App 写下的真实数据/附件路径（uninstall-info.txt），默认保留数据；
;      用户明确选择后才删除 App 自有目录。
;   3. 自定义附件目录只提示、不删除，避免误删共享目录。
;
; 静默卸载（/S）不做任何交互，按「保留数据」处理。
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

  ; 2) 展示真实路径，问是否删除数据（默认保留）
  Call un.wsWbReadInfo
  ${If} $wsWbInfo == ""
    StrCpy $wsWbInfo "未找到路径清单（uninstall-info.txt），将只删除程序文件，保留用户数据。"
  ${Else}
    StrCpy $wsWbInfo "以下为 ws-workbench 的关联路径：$\r$\n$wsWbInfo"
  ${EndIf}
  ${If} $wsWbCustom == "1"
    StrCpy $wsWbInfo "$wsWbInfo$\r$\n$\r$\n注意：检测到自定义附件目录。卸载程序不会删除它，请按上面的 upload_dir 自行处理。"
  ${EndIf}

  MessageBox MB_YESNO|MB_DEFBUTTON2|MB_ICONQUESTION "$wsWbInfo$\r$\n$\r$\n是否同时删除 ws-workbench 的用户数据？$\r$\n删除后附件、数据库、剪贴板图片不可恢复。选择「否」将保留数据，只卸载程序。" IDYES ws_wb_delete IDNO ws_wb_keep

  ws_wb_delete:
    Push "用户选择删除用户数据"
    Call un.wsWbLog
    RMDir /r "$APPDATA\com.ws.workbench"
    RMDir /r "$APPDATA\ws-workbench"
    RMDir /r "$LOCALAPPDATA\com.ws.workbench"
    RMDir /r "$LOCALAPPDATA\ws-workbench"
    RMDir /r "$PROFILE\.ws-workbench"
    Goto ws_wb_done

  ws_wb_keep:
    Push "用户选择保留用户数据"
    Call un.wsWbLog

  ws_wb_done:

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
