; ws-workbench 的 NSIS 简体中文文案。
;
; 与 tauri-bundler 的 English.nsh 逐项对应：安装器里所有自定义字符串都在这里，
; 缺任何一条 makensis 都会报「undefined language string」。
; 通过 tauri.conf.json 的 bundle.windows.nsis.customLanguageFiles 引入，
; 必须带 UTF-8 BOM，NSIS 3 才会按 Unicode 解析中文。
;
; `{{product_name}}` 由打包器替换成产品名（与上游 English.nsh 一致）。

LangString addOrReinstall ${LANG_SIMPCHINESE} "添加 / 重新安装组件"
LangString alreadyInstalled ${LANG_SIMPCHINESE} "已安装"
LangString alreadyInstalledLong ${LANG_SIMPCHINESE} "${PRODUCTNAME} ${VERSION} 已经安装过了。请选择要执行的操作，然后点「下一步」继续。"
LangString appRunning ${LANG_SIMPCHINESE} "{{product_name}} 正在运行，请先退出它再重试。"
LangString appRunningOkKill ${LANG_SIMPCHINESE} "{{product_name}} 正在运行！$\n点「确定」结束它。"
LangString chooseMaintenanceOption ${LANG_SIMPCHINESE} "请选择要执行的维护操作。"
LangString chooseHowToInstall ${LANG_SIMPCHINESE} "请选择 ${PRODUCTNAME} 的安装方式。"
LangString createDesktop ${LANG_SIMPCHINESE} "创建桌面快捷方式"
LangString dontUninstall ${LANG_SIMPCHINESE} "不卸载"
LangString dontUninstallDowngrade ${LANG_SIMPCHINESE} "不卸载（本安装程序不允许直接降级）"
LangString failedToKillApp ${LANG_SIMPCHINESE} "无法结束 {{product_name}}，请先手动退出它再重试。"
LangString installingWebview2 ${LANG_SIMPCHINESE} "正在安装 WebView2…"
LangString newerVersionInstalled ${LANG_SIMPCHINESE} "已经安装了更高版本的 ${PRODUCTNAME}！不建议再装旧版本；确实要装的话，建议先卸载当前版本。请选择操作后点「下一步」继续。"
LangString older ${LANG_SIMPCHINESE} "更早"
LangString olderOrUnknownVersionInstalled ${LANG_SIMPCHINESE} "系统里已安装 $R4 版本的 ${PRODUCTNAME}，建议先卸载当前版本再安装。请选择操作后点「下一步」继续。"
LangString silentDowngrades ${LANG_SIMPCHINESE} "本安装程序禁止降级，无法静默安装；请改用图形界面安装。$\n"
LangString unableToUninstall ${LANG_SIMPCHINESE} "无法卸载！"
LangString uninstallApp ${LANG_SIMPCHINESE} "卸载 ${PRODUCTNAME}"
LangString uninstallBeforeInstalling ${LANG_SIMPCHINESE} "安装前先卸载"
LangString unknown ${LANG_SIMPCHINESE} "未知"
LangString webview2AbortError ${LANG_SIMPCHINESE} "安装 WebView2 失败，缺少它应用无法运行。请重启安装程序后重试。"
LangString webview2DownloadError ${LANG_SIMPCHINESE} "错误：下载 WebView2 失败 — $0"
LangString webview2DownloadSuccess ${LANG_SIMPCHINESE} "WebView2 引导程序下载完成"
LangString webview2Downloading ${LANG_SIMPCHINESE} "正在下载 WebView2 引导程序…"
LangString webview2InstallError ${LANG_SIMPCHINESE} "错误：安装 WebView2 失败，退出码 $1"
LangString webview2InstallSuccess ${LANG_SIMPCHINESE} "WebView2 安装完成"
LangString deleteAppData ${LANG_SIMPCHINESE} "同时删除应用数据（设置、数据库、缓存）"
