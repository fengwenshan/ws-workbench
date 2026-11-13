#!/usr/bin/env bash
# 构建 macOS 内置的 openconnect（静态 GnuTLS，运行时只依赖 macOS 系统库）。
#
# 用法：
#   scripts/build-openconnect-macos.sh                 # 通用二进制（arm64 + x86_64）
#   scripts/build-openconnect-macos.sh --arch arm64    # 只编 arm64
#   scripts/build-openconnect-macos.sh --arch x86_64   # 在 Apple Silicon 上也能交叉编
#   scripts/build-openconnect-macos.sh --out /tmp/oc   # 指定输出路径
#
# 产物默认写到 src-tauri/resources/openconnect-macos/openconnect，
# 打包时由 src-tauri/tauri.macos.conf.json 映射到 Contents/Resources/apps/openconnect/。
#
# 构建依赖（都只在构建机需要）：
#   - Xcode Command Line Tools：clang / make / libtool / xcrun
#   - pkg-config（macOS 不自带，`brew install pkgconf` 即可）
#   - curl、tar、shasum
# 不需要 Homebrew 的 gnutls / nettle / gmp，也不需要本机装 openconnect。
#
# 依赖版本与源包 SHA256 固定在下面；改了版本就要同步更新
# src-tauri/resources/openconnect-macos/README.md 里的溯源信息。
set -euo pipefail

GMP_VERSION="6.3.0"
NETTLE_VERSION="4.0"
GNUTLS_VERSION="3.8.13"
OPENCONNECT_VERSION="9.21"

GMP_SHA256="a3c2b80201b89e68616f4ad30bc66aee4927c3ce50e33929ca819d5c43538898"
NETTLE_SHA256="3addbc00da01846b232fb3bc453538ea5468da43033f21bb345cb1e9073f5094"
GNUTLS_SHA256="ffed8ec1bf09c2426d4f14aae377de4753b53e537d685e604e99a8b16ca9c97e"
OPENCONNECT_SHA256="5b32369467db6e5f317aa1ed12cfcbb81ed00bdbc765450b6bfcbdc300944a58"

MIRROR="${OC_SOURCE_MIRROR:-https://ftp.gnu.org}"
GNUTLS_URL="${OC_GNUTLS_URL:-https://www.gnupg.org/ftp/gcrypt/gnutls/v3.8/gnutls-${GNUTLS_VERSION}.tar.xz}"
OPENCONNECT_URL="${OC_OPENCONNECT_URL:-https://www.infradead.org/openconnect/download/openconnect-${OPENCONNECT_VERSION}.tar.gz}"

# 与 README「平台与限制」一致：arm64 的 macOS 最低就是 11.0，Intel 也按 11.0 编，
# 避免依赖只在新系统上存在的符号。
MIN_MACOS="11.0"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_ROOT="${OC_BUILD_DIR:-$REPO_ROOT/.tmp/openconnect-build}"
OUT="$REPO_ROOT/src-tauri/resources/openconnect-macos/openconnect"
ARCH="universal"

usage() {
  sed -n '2,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

die() {
  echo "错误：$*" >&2
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="${2:-}"; shift 2 ;;
    --out) OUT="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) die "未知参数：$1（用 --help 看用法）" ;;
  esac
done
case "$ARCH" in
  arm64|x86_64|universal) ;;
  *) die "--arch 只能是 arm64 / x86_64 / universal" ;;
esac
[[ "$OUT" = /* ]] || OUT="$REPO_ROOT/$OUT"

for tool in clang make libtool lipo strip codesign xcrun curl tar shasum; do
  command -v "$tool" >/dev/null 2>&1 || die "缺少构建工具：$tool"
done
PKG_CONFIG_BIN="$(command -v pkg-config || command -v pkgconf || true)"
[[ -n "$PKG_CONFIG_BIN" ]] || die "缺少 pkg-config（macOS 不自带，执行 brew install pkgconf）"

NPROC="$(sysctl -n hw.ncpu)"
HOST_ARCH="$(uname -m)"
case "$HOST_ARCH" in
  arm64) HOST_GNU=aarch64 ;;
  x86_64) HOST_GNU=x86_64 ;;
  *) die "不支持的构建机架构：$HOST_ARCH" ;;
esac

SRC_DIR="$BUILD_ROOT/src"
PC_DIR="$BUILD_ROOT/pc"
mkdir -p "$SRC_DIR" "$PC_DIR" "$BUILD_ROOT/logs"

# SDK 里的 zlib / libxml2 只有头文件与系统库，没有 pkg-config 文件；
# 自己写两份最小 .pc，让 GnuTLS / openconnect 的 configure 能按常规路径找到它们。
SDK_PATH="$(xcrun --show-sdk-path)"
cat > "$PC_DIR/zlib.pc" <<'EOF'
Name: zlib
Description: zlib compression library (macOS SDK)
Version: 1.2.13
Libs: -lz
Cflags:
EOF
cat > "$PC_DIR/libxml-2.0.pc" <<EOF
Name: libxml-2.0
Description: libxml2 library (macOS SDK)
Version: 2.9.13
Libs: -lxml2
Cflags: -I$SDK_PATH/usr/include/libxml2
EOF

fetch() {
  local name="$1" url="$2" sha="$3" out="$SRC_DIR/$1"
  if [[ ! -f "$out" ]]; then
    echo "下载 $name → $out"
    curl -fL --retry 5 --retry-delay 2 --connect-timeout 20 -o "$out.part" "$url"
    mv "$out.part" "$out"
  fi
  local actual
  actual="$(shasum -a 256 "$out" | awk '{print $1}')"
  [[ "$actual" = "$sha" ]] || die "$name 校验失败：期望 $sha，实际 $actual（删掉 $out 重试）"
}

prepare_sources() {
  fetch "gmp-${GMP_VERSION}.tar.xz" "$MIRROR/gnu/gmp/gmp-${GMP_VERSION}.tar.xz" "$GMP_SHA256"
  fetch "nettle-${NETTLE_VERSION}.tar.gz" "$MIRROR/gnu/nettle/nettle-${NETTLE_VERSION}.tar.gz" "$NETTLE_SHA256"
  fetch "gnutls-${GNUTLS_VERSION}.tar.xz" "$GNUTLS_URL" "$GNUTLS_SHA256"
  fetch "openconnect-${OPENCONNECT_VERSION}.tar.gz" "$OPENCONNECT_URL" "$OPENCONNECT_SHA256"

  for pkg in "gmp-${GMP_VERSION}" "nettle-${NETTLE_VERSION}" "gnutls-${GNUTLS_VERSION}" "openconnect-${OPENCONNECT_VERSION}"; do
    if [[ ! -d "$SRC_DIR/$pkg" ]]; then
      echo "解压 $pkg"
      tar -xf "$SRC_DIR/$pkg".tar.* -C "$SRC_DIR"
    fi
  done
}

# 交叉编译时 configure 需要同时知道 build/host；本机架构直接用 host 即可。
configure_target_flags() {
  local arch="$1"
  local host_flag="--host=$arch-apple-darwin"
  if [[ "$arch" != "$HOST_ARCH" ]]; then
    host_flag="$host_flag --build=$HOST_GNU-apple-darwin"
  fi
  echo "$host_flag"
}

build_arch() {
  local arch="$1"
  local root="$BUILD_ROOT/arch-$arch"
  local stage="$root/stage"
  local log="$BUILD_ROOT/logs/arch-$arch.log"
  local host_flag
  host_flag="$(configure_target_flags "$arch")"

  echo "==> 构建 ${arch}（日志：${log}）"
  rm -rf "$root"
  mkdir -p "$stage" "$root/build"

  local cc_flags="-O2 -g0 -arch $arch -mmacosx-version-min=$MIN_MACOS"
  local common_env=(
    "MACOSX_DEPLOYMENT_TARGET=$MIN_MACOS"
    "CC=clang -arch $arch"
    "CXX=clang++ -arch $arch"
    "CFLAGS=$cc_flags"
    "CXXFLAGS=$cc_flags"
    "LDFLAGS=-arch $arch -mmacosx-version-min=$MIN_MACOS"
    "PKG_CONFIG=$PKG_CONFIG_BIN"
  )

  # shellcheck disable=SC2086
  env "${common_env[@]}" bash -euo pipefail -c '
    set -euo pipefail
    SRC="$1" STAGE="$2" ROOT="$3" LOG="$4" ARCH="$5" HOST_FLAG="$6" PC_DIR="$7" NPROC="$8"
    exec >>"$LOG" 2>&1
    echo "===== build $ARCH ====="
    mkdir -p "$ROOT/build/gmp"
    cd "$ROOT/build/gmp"
    "$SRC/gmp-'"$GMP_VERSION"'/configure" $HOST_FLAG --disable-assembly --disable-shared --enable-static --prefix="$STAGE/gmp"
    make -j"$NPROC"
    make install

    mkdir -p "$ROOT/build/nettle"
    cd "$ROOT/build/nettle"
    "$SRC/nettle-'"$NETTLE_VERSION"'/configure" $HOST_FLAG --disable-assembler --disable-shared --enable-static --disable-documentation --prefix="$STAGE/nettle" CPPFLAGS="-I$STAGE/gmp/include" LDFLAGS="-L$STAGE/gmp/lib $LDFLAGS"
    make -j"$NPROC"
    make install

    mkdir -p "$ROOT/build/gnutls"
    cd "$ROOT/build/gnutls"
    PKG_CONFIG_PATH="$STAGE/nettle/lib/pkgconfig:$STAGE/gmp/lib/pkgconfig:$PC_DIR" \
      "$SRC/gnutls-'"$GNUTLS_VERSION"'/configure" $HOST_FLAG \
        --prefix="$STAGE/gnutls" \
        --disable-shared --enable-static \
        --disable-doc --disable-tests --disable-tools --disable-guile --disable-nls \
        --disable-hardware-acceleration \
        --without-p11-kit --without-tpm --without-tpm2 --disable-libdane \
        --without-zstd --without-brotli \
        --with-included-libtasn1 --with-included-unistring --without-idn \
        --with-default-trust-store-file=/etc/ssl/cert.pem \
        CPPFLAGS="-I$STAGE/gmp/include -I$STAGE/nettle/include" \
        LDFLAGS="-L$STAGE/gmp/lib -L$STAGE/nettle/lib $LDFLAGS"
    make -j"$NPROC"
    make install
    # 删掉 libtool 的 .la：它会把 Homebrew / 构建机上的绝对路径带进后续链接。
    rm -f "$STAGE/gnutls/lib"/*.la

    mkdir -p "$ROOT/build/openconnect"
    cd "$ROOT/build/openconnect"
    PKG_CONFIG_PATH="$STAGE/gnutls/lib/pkgconfig:$PC_DIR" \
      GNUTLS_CFLAGS="-I$STAGE/gnutls/include" \
      GNUTLS_LIBS="-L$STAGE/gnutls/lib -lgnutls -L$STAGE/nettle/lib -lhogweed -lnettle -L$STAGE/gmp/lib -lgmp -lz -liconv -framework Security -framework CoreFoundation" \
      "$SRC/openconnect-'"$OPENCONNECT_VERSION"'/configure" $HOST_FLAG \
        --prefix="$STAGE/openconnect" \
        --disable-shared --enable-static --disable-nls \
        --without-stoken --without-p11-kit --without-libpskc --without-libpcsclite \
        --without-gssapi --without-lz4 --without-libproxy \
        --with-builtin-json \
        --with-vpnc-script=/usr/local/share/vpnc-scripts/vpnc-script
    make -j"$NPROC" openconnect

    cd "$ROOT/build/openconnect"
    if otool -L openconnect | grep -q "/opt/homebrew\|/usr/local/opt"; then
      echo "链接结果仍带 Homebrew 动态库：" >&2
      otool -L openconnect >&2
      exit 1
    fi
    ./openconnect --version | grep -q "OpenConnect version v'"$OPENCONNECT_VERSION"'" || {
      echo "openconnect --version 不符合预期" >&2
      ./openconnect --version >&2
      exit 1
    }
    lipo -info openconnect | grep -q "$ARCH"
  ' _ "$SRC_DIR" "$stage" "$root" "$log" "$arch" "$host_flag" "$PC_DIR" "$NPROC" || {
    echo "=== $arch 构建失败，日志末尾 ===" >&2
    tail -n 40 "$log" >&2 || true
    exit 1
  }

  echo "    $arch 构建完成"
}

prepare_sources
mkdir -p "$(dirname "$OUT")"

case "$ARCH" in
  universal)
    build_arch arm64
    build_arch x86_64
    echo "==> 合并通用二进制"
    lipo -create "$BUILD_ROOT/arch-arm64/build/openconnect/openconnect" \
                 "$BUILD_ROOT/arch-x86_64/build/openconnect/openconnect" \
                 -output "$OUT"
    ;;
  *)
    build_arch "$ARCH"
    cp "$BUILD_ROOT/arch-$ARCH/build/openconnect/openconnect" "$OUT"
    ;;
esac

chmod 755 "$OUT"
# 去掉调试/本地符号（-x 保留导出符号，openconnect 不需要可调试符号）；
# 再补一个 ad-hoc 签名，否则 macOS 上对未签名 Mach-O 的执行/打包检查会报警。
strip -x "$OUT"
codesign --force --sign - --timestamp=none "$OUT" >/dev/null 2>&1 || true

echo "==> 校验产物"
lipo -info "$OUT"
otool -L "$OUT"
"$OUT" --version
shasum -a 256 "$OUT"
echo "已写入：$OUT"
