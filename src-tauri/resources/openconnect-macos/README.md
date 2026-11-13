# macOS 内置 openconnect

这个目录是 macOS 安装包自带的 openconnect 运行时：一个**通用二进制**（arm64 + x86_64），
静态链接 GnuTLS，运行时只依赖 macOS 系统库。macOS 版 VPN 只认这一份，
不再读 Homebrew、PATH 或用户指定的外部副本（见 `src-tauri/src/vpn/platform/macos.rs`）。

## 打包位置

`src-tauri/tauri.macos.conf.json` 在构建时做资源映射：

```json
"resources/openconnect-macos": "apps/openconnect"
```

所以安装包里它是 `ws-workbench.app/Contents/Resources/apps/openconnect/openconnect`，
`apps::bundled_binary(AppId::Openconnect, ctx)` 找不到时会报「安装包缺少内置 openconnect」。
调试构建（`pnpm tauri dev` / `cargo run`）资源还没拷进 target 时，会回退到本目录的
`openconnect`（`CARGO_MANIFEST_DIR/resources/openconnect-macos/openconnect`）。

## 版本与溯源

| 组件 | 版本 | 说明 |
| --- | --- | --- |
| openconnect | 9.21 | [上游源码](https://www.infradead.org/openconnect/download/openconnect-9.21.tar.gz)，SHA256 `5b32369467db6e5f317aa1ed12cfcbb81ed00bdbc765450b6bfcbdc300944a58` |
| GnuTLS | 3.8.13 | [上游源码](https://www.gnupg.org/ftp/gcrypt/gnutls/v3.8/gnutls-3.8.13.tar.xz)，SHA256 `ffed8ec1bf09c2426d4f14aae377de4753b53e537d685e604e99a8b16ca9c97e` |
| nettle | 4.0 | [上游源码](https://ftp.gnu.org/gnu/nettle/nettle-4.0.tar.gz)，SHA256 `3addbc00da01846b232fb3bc453538ea5468da43033f21bb345cb1e9073f5094` |
| GMP | 6.3.0 | [上游源码](https://ftp.gnu.org/gnu/gmp/gmp-6.3.0.tar.xz)，SHA256 `a3c2b80201b89e68616f4ad30bc66aee4927c3ce50e33929ca819d5c43538898` |

- 产物：`openconnect`（fat Mach-O：arm64 + x86_64），约 5.8 MB
- SHA256：`3adaaf8cb1f6e65964b868bf833e8fe317fb08db2733b9ffec41cf016c3e88f7`
- 最低系统：`LC_BUILD_VERSION minos 11.0`（arm64 本身最低 11.0；Intel 也按 11.0 编）
- 编译选项：`--disable-shared --enable-static`、`--disable-nls`、
  `--without-stoken --without-p11-kit --without-libpskc --without-libpcsclite`、
  `--with-builtin-json`、`--with-included-libtasn1 --with-included-unistring --without-idn`、
  GnuTLS 关闭硬件加速与 p11-kit/tpm/tpm2（`--disable-hardware-acceleration`）
- 信任库：`--with-default-trust-store-file=/etc/ssl/cert.pem`（macOS 系统自带的公开 CA 包）。
  企业内网自建 CA / 钥匙串里手动信任的根证书不会被这份 openconnect 信任（Homebrew
  那份同样只认它自己的 cert.pem），需要时把根证书合并进一个 PEM 后用 `--cafile` 传入（当前
  界面暂未暴露这个参数）。
- 功能：`HOTP software token, TOTP software token, System keys, DTLS, ESP`；
  协议 `anyconnect (default), nc, gp, pulse, f5, fortinet, array`。
  没有 PKCS#11/智能卡和 Yubikey OATH —— 本应用只用「用户名 + 密码」（走 stdin）和
  `--authgroup` / `--servercert` 这类参数，用不到这两块。

### 为什么要自建

Homebrew 版 openconnect 会带一串 `/opt/homebrew` 下的 dylib（libgnutls、libnettle、
libgmp、libp11-kit……），而且现在的瓶装库最低系统已经是 macOS 26；用户还得自己
`brew install openconnect`。自建静态版把这串依赖收进一个文件，顺便把最低系统拉回 11.0。

## 重新构建

前置：Xcode Command Line Tools（clang / make / libtool / xcrun）+ `pkg-config`
（macOS 不自带，`brew install pkgconf` 即可；只需要这一个构建期工具，不需要 brew 的
gnutls/nettle/gmp，也不需要本机装 openconnect）+ curl / tar / shasum。

```bash
# 在仓库根目录执行；默认产出通用二进制并直接覆盖本目录的 openconnect
scripts/build-openconnect-macos.sh

# 只编一个架构 / 换输出路径
scripts/build-openconnect-macos.sh --arch arm64
scripts/build-openconnect-macos.sh --arch x86_64 --out /tmp/openconnect-x86
```

脚本会下载并校验上面四个源码包（缓存到 `.tmp/openconnect-build/`，可复用），
按 `-mmacosx-version-min=11.0` 从源码依次构建 GnuTLS（静态）和 openconnect（静态），
最后 `lipo` 合并、`strip -x`、ad-hoc `codesign -s -`，并打印 `otool -L` 与 SHA256。
在 Apple Silicon 上交叉编 x86_64 已验证可用。

几个踩过的坑（脚本里已处理，手工构建时注意）：

1. **libtool 会把绝对路径的 `.a` 塞进自己的静态归档**，导致 `libopenconnect.a` 只有 96 字节，
   报 `Undefined symbols: _openconnect_vpninfo_new`。所有依赖库必须用
   `-L<dir> -l<name>` 形式传入，不能写 `.a` 的绝对路径。
2. 构建 GnuTLS 时 `LDFLAGS` / `CPPFLAGS` 必须显式带 gmp、nettle 的 `-L` / `-I`，
   否则报 `*** gmp was not found`。
3. 安装 GnuTLS 后要删掉 stage 里的 `lib/*.la`，否则它会把构建机的 Homebrew 路径带进
   openconnect 的链接。
4. GnuTLS 3.8.11 与 nettle 4.0 不兼容（4.0 删了 `nettle/sha.h`），必须用 ≥ 3.8.13。
5. macOS 没有自带 `pkg-config`，脚本要求先装；GnuTLS 的 configure 缺它直接报错。
6. 链接完成后 `strip` 会使 ad-hoc 签名失效，必须重新 `codesign --force --sign -`。

## 验证

```bash
cd src-tauri/resources/openconnect-macos
lipo -info openconnect              # x86_64 arm64
otool -L openconnect                # 只应有 /usr/lib 与 /System/Library/Frameworks
./openconnect --version             # OpenConnect version v9.21 / GnuTLS 3.8.13
env -i ./openconnect --version      # 不依赖任何 shell 环境
shasum -a 256 openconnect           # 与上面 SHA256 一致
```

`cargo test --lib vpn::platform::macos` 里有一个用例会直接跑这份二进制，产物缺失或
跑不起来都会失败。

## 许可

按上游许可证分发；`LICENSES/` 里放了完整文本：

- openconnect：LGPL-2.1（`LICENSES/openconnect-LGPL-2.1.txt`）
- GnuTLS：LGPL-2.1+（`LICENSES/gnutls-LGPL-2.1.txt`）/ GPL-3.0（`LICENSES/gnutls-GPL-3.0.txt`）
- nettle：LGPL-3.0+ / GPL-3.0（`LICENSES/nettle-*.txt`）
- GMP：LGPL-3.0+ / GPL-3.0（`LICENSES/gmp-*.txt`）

本仓库是公开的，构建脚本与补丁都可获取；按 LGPL 的要求，下游可以用
`scripts/build-openconnect-macos.sh` 重建一份并替换安装包里的这个可执行文件。

## 升级步骤

1. 改 `scripts/build-openconnect-macos.sh` 顶部的版本号与 SHA256。
2. 跑一次脚本重新生成 `openconnect`，确认 `--version`、功能列表和 `otool -L` 没退化。
3. 同步更新本文件的版本表、SHA256 与体积；如果上游改了协议/功能，也更新上面「功能」一节。
4. 跑 `cargo test --lib vpn`（含内置产物冒烟用例）与 `pnpm build`。
