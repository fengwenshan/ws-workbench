# 在 Windows 上跑 cargo test 的包装脚本。
#
# 为什么需要它：VPN 的关机拦截链接进了 comctl32 v6 才有的导入（TaskDialogIndirect 等），
# 而 cargo 产出的测试 exe 没有应用清单，加载器拿到 system32 里的 v5.82 会直接以
# 0xc0000139（STATUS_ENTRYPOINT_NOT_FOUND）退出——表现成「一个测试都没跑就失败」。
# 这里给测试目标补一份声明 comctl32 v6 依赖的清单（scripts/test-comctl.manifest）。
#
# 用法：
#   pwsh scripts/test-windows.ps1                # 默认 --lib
#   pwsh scripts/test-windows.ps1 lanshare       # cargo test --lib lanshare
#
# 注意：RUSTFLAGS 变了会触发一次全量重编译（cargo 的指纹包含编译参数），
# 只影响本脚本的这次运行，不会污染正常构建产物。

param([Parameter(ValueFromRemainingArguments = $true)][string[]]$CargoArgs)

$manifest = Join-Path $PSScriptRoot 'test-comctl.manifest'
if (-not (Test-Path -LiteralPath $manifest)) {
    Write-Error "找不到清单: $manifest"
    exit 1
}
if ($manifest -match '\s') {
    Write-Error "仓库路径带空格时 RUSTFLAGS 会被拆开，请把仓库放到无空格路径: $manifest"
    exit 1
}

$env:RUSTFLAGS = (@($env:RUSTFLAGS, "-C link-arg=/MANIFEST:EMBED", "-C link-arg=/MANIFESTINPUT:$manifest") |
        Where-Object { $_ }) -join ' '

if (-not $CargoArgs -or $CargoArgs.Count -eq 0) {
    $CargoArgs = @('--lib')
}

Push-Location (Join-Path $PSScriptRoot '..\src-tauri')
try {
    Write-Host "RUSTFLAGS=$env:RUSTFLAGS"
    & cargo test @CargoArgs
    exit $LASTEXITCODE
} finally {
    Pop-Location
}
