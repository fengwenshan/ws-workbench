# macOS 内置 openconnect（静态、只认内置）设计

**日期：** 2026-11-13
**状态：** 已实现（分支 `feat/vpn`）

## 背景

macOS 版此前和 Windows 不一致：Windows 安装包自带 `openconnect.exe` + DLL，macOS 却要求用户
自己 `brew install openconnect`，程序再去 `/opt/homebrew`、`/usr/local`、PATH 或用户指定的目录里找。
线上还踩过 Homebrew 自定义前缀不被识别、复制副本权限被拒、`brew upgrade` 后路径失配等问题；
更现实的是 Homebrew 现在的 `openconnect` / `libgnutls` 瓶装库已经按 macOS 26 构建，用户装了也未必能跑。

用户要求：**macOS 也把 openconnect 内置进应用，不再依赖 brew**，并且发布版和调试版都
**只认内置，任何构建都不回退**到系统安装的那份。

## 决策

1. **只认内置**：`macos.rs` 的唯一候选是 `<资源目录>/apps/openconnect/openconnect`。
   不读 Homebrew / 标准前缀 / PATH / 用户路径；不提供「选择安装目录」入口；
   导入命令直接报错。旧版本留下的路径登记与副本在检测时清掉。
2. **静态链接 GnuTLS，自建依赖**：不打包 Homebrew 的 dylib 闭包（那些库最低系统是 macOS 26，
   且要维护 10 个文件 + rpath）。改为从源码构建
   GMP + nettle + GnuTLS + openconnect，全部 `--disable-shared --enable-static`，
   最终只依赖 `/usr/lib` 与 `/System/Library/Frameworks`。
3. **通用二进制**：arm64 + x86_64 `lipo` 合并，`minos 11.0`（arm64 的硬下限；Intel 也对齐到 11.0）。
   产物连同 `scripts/build-openconnect-macos.sh`、许可证一起提交，安装包不再需要构建期拉取。
4. **调试构建可用**：资源还没拷进 target 时回退到源码树
   `src-tauri/resources/openconnect-macos/openconnect`（`cfg(test)` 下关闭，便于单测造资源目录）；
   release 不带这条回退。

### 为什么选 GnuTLS 而不是 OpenSSL

Windows 版本来就用 GnuTLS；GnuTLS 版保留了 macOS「System keys」支持，体积也更小
（静态产物约 2.9MB/架构 vs OpenSSL 5.6MB/架构）。本应用只用用户名 + 密码（stdin）+ 额外参数，
不需要 PKCS#11/智能卡，因此 GnuTLS 缺的这块不影响功能。

## 产物事实

- openconnect 9.21，GnuTLS 3.8.13，nettle 4.0，GMP 6.3.0（libtasn1/unistring 用 GnuTLS 自带副本，
  `--without-idn`、`--without-p11-kit`、`--without-tpm`、`--without-libpcsclite`、`--with-builtin-json`）
- 通用 Mach-O，约 5.8MB，SHA256 记录在资源目录 README；`LC_BUILD_VERSION minos 11.0`
- `otool -L` 只有 `libz`、`libxml2`、`libiconv`、CoreFoundation、Security、libSystem
- 功能：HOTP/TOTP software token、System keys、DTLS、ESP；协议 anyconnect/nc/gp/pulse/f5/fortinet/array
- 信任库：`/etc/ssl/cert.pem`（公开 CA）；企业自签根证书仍需 `--cafile`（界面暂未暴露）
- 构建细节与踩坑记录：`src-tauri/resources/openconnect-macos/README.md`

## 改动点

- `src-tauri/src/vpn/platform/macos.rs`：重写为只认内置；显式用无后缀的 `openconnect`
  （避免 `apps::bundled_binary` 的 `openconnect.exe` 候选把 Windows 文件误当内置副本）；
  旧登记/副本清理；导入拒绝；单测覆盖内置探测、缺失提示、忽略自定义路径、旧数据清理、
  C locale、源码树产物冒烟。
- `src-tauri/tauri.macos.conf.json`：`"resources/openconnect-macos": "apps/openconnect"`；
  保留 `"resources/apps": null`；`bundle.macOS.minimumSystemVersion = "11.0"`。
- `src-tauri/resources/openconnect-macos/`：通用二进制、README、LICENSES。
- `scripts/build-openconnect-macos.sh`：可复现构建（校验上游源码 SHA256、交叉编 x86_64、lipo、
  strip、ad-hoc 签名、自检 `otool -L` / `--version`）。
- `vpn/guide.rs`：macOS 成功态/缺失态文案改为「安装包内置」，删除 brew 命令与链接。
- 前端 `VpnPanel.vue` / `VpnInstallGuide.vue` / `useVpn.ts`：导入入口只在 `platform.is_linux`
  （当前两个平台都不显示），文案去 Homebrew 化。
- README、`resources/apps/openconnect/README.md`、平台层设计文档同步更新。

## 验证

- `cargo test --lib`：545 passed（含内置产物冒烟用例：源码树二进制存在、能跑、版本 v9.21）。
- `pnpm build` 通过。
- `tauri build --debug --bundles app`：`Contents/Resources/apps/openconnect/openconnect`
  存在且 `--version` 正常；无 `openconnect.exe` / `wintun.dll`；`Info.plist` 为 11.0。
- 构建脚本分别跑通 arm64 与 x86_64 单架构，产物 `otool -L` 均无 `/opt/homebrew`；
  x86_64 切片在 Apple Silicon 上经 Rosetta 能执行 `--version`。

## 风险与限制

- 未在 macOS 11–13 真机上以 root 建立真实 utun 隧道验证；已验证 TLS/CLI、系统库依赖与提权路径的既有测试。
- 只信任公开 CA；企业内部根证书仍需要用户提供 `--cafile`（后续可在 VPN 配置里加）。
- 无 PKCS#11/智能卡、Yubikey OATH（本应用用不到）。
- 二进制进 git（约 5.8MB）；升级时用脚本重建并同步 README 的版本/哈希。
- 未签名/未公证的发布方式不变；嵌套的 openconnect 已做 ad-hoc 签名，将来若接入 Developer ID
  公证，需要一并签名这个嵌套可执行文件。

## 不在这一轮

- Developer ID 签名与公证。
- PKCS#11 / 智能卡支持。
- Linux 支持。
