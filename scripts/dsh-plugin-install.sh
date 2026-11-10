#!/usr/bin/env bash
# 从本地 zip 安装 / 卸载 DeepSeek 工作区插件（开发 / 离线用；生产走 ws-workbench 设置 → 资源）。
#
# 用法：
#   scripts/dsh-plugin-install.sh --zip dsh-open-workspace-0.1.0.zip
#   scripts/dsh-plugin-install.sh --uninstall
#
# 环境变量：DSH_HOME（默认 ~/.dsh）
set -euo pipefail

DSH_HOME="${DSH_HOME:-$HOME/.dsh}"
PROFILE_DIR="$DSH_HOME/profiles/desktop"
PLUGIN_NAME="dsh-open-workspace"
STORE_DIR="$DSH_HOME/plugins/$PLUGIN_NAME"
ACTIVE_DIR="$PROFILE_DIR/node_modules/$PLUGIN_NAME"
MANIFEST="$PROFILE_DIR/package.json"

die() { echo "错误: $*" >&2; exit 1; }

uninstall() {
  rm -rf "$STORE_DIR" "$ACTIVE_DIR"
  python3 - "$MANIFEST" <<'PYEOF'
import json, sys
path = sys.argv[1]
with open(path) as f:
    data = json.load(f)
data.get("dependencies", {}).pop("dsh-open-workspace", None)
bundles = data.get("dsh", {}).get("profile", {}).get("bundles", [])
if "dsh-open-workspace" in bundles:
    bundles.remove("dsh-open-workspace")
with open(path, "w") as f:
    json.dump(data, f, ensure_ascii=False, indent=2)
PYEOF
  echo "已卸载。重启 DeepSeek Harness 生效。"
}

install_zip() {
  local zip="$1"
  [ -f "$zip" ] || die "找不到 zip: $zip"
  [ -f "$MANIFEST" ] || die "DeepSeek 还没初始化：先打开一次 DeepSeek Harness"
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  unzip -q "$zip" -d "$tmp/dsh-open-workspace"
  for f in package.json cordis.patch.yml lib/index.mjs lib/client.js README.md; do
    [ -f "$tmp/dsh-open-workspace/$f" ] || die "zip 缺少 $f"
  done
  mkdir -p "$STORE_DIR" "$ACTIVE_DIR"
  cp -R "$tmp/dsh-open-workspace/." "$STORE_DIR/"
  cp -R "$tmp/dsh-open-workspace/." "$ACTIVE_DIR/"
  [ -f "$MANIFEST.ws-workbench.bak" ] || cp "$MANIFEST" "$MANIFEST.ws-workbench.bak"
  python3 - "$MANIFEST" <<'PYEOF'
import json, sys
path = sys.argv[1]
with open(path) as f:
    data = json.load(f)
data.setdefault("dependencies", {})["dsh-open-workspace"] = "link:../../plugins/dsh-open-workspace"
bundles = data.setdefault("dsh", {}).setdefault("profile", {}).setdefault("bundles", [])
if "dsh-open-workspace" not in bundles:
    bundles.append("dsh-open-workspace")
with open(path, "w") as f:
    json.dump(data, f, ensure_ascii=False, indent=2)
PYEOF
  echo "已安装。重启 DeepSeek Harness 生效。"
}

case "${1:-}" in
  --zip) install_zip "${2:?--zip 需要 zip 路径}" ;;
  --uninstall) uninstall ;;
  *) die "用法: $0 --zip <path> | --uninstall" ;;
esac
