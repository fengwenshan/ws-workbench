#!/usr/bin/env bash
# 把 dsh-open-workspace 插件装进本机 DeepSeek Harness 的 desktop profile。
#
# 生产用户不需要手动跑：ws-workbench 点「打开方式 → DeepSeek」时会自动安装（弹窗确认）。
# 这个脚本用于：开发机手动安装、CI/临时 DSH_HOME、排障卸载。
#
# 用法：
#   scripts/dsh-plugin-install.sh            # 安装 / 更新
#   scripts/dsh-plugin-install.sh --uninstall
#
# 环境变量：DSH_HOME（默认 ~/.dsh）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/dsh-plugin/dsh-open-workspace"
DSH_HOME="${DSH_HOME:-$HOME/.dsh}"
PROFILE="$DSH_HOME/profiles/desktop"
MANIFEST="$PROFILE/package.json"
BACKUP="$PROFILE/package.json.ws-workbench.bak"
PLUGIN_NAME="dsh-open-workspace"

if [ ! -f "$MANIFEST" ]; then
  echo "找不到 $MANIFEST —— 先打开一次 DeepSeek Harness 让它初始化 desktop profile。" >&2
  exit 1
fi

if [ "${1:-}" = "--uninstall" ]; then
  python3 - "$MANIFEST" "$PLUGIN_NAME" <<'PY'
import json, sys
path, name = sys.argv[1], sys.argv[2]
with open(path) as f:
    data = json.load(f)
data.get("dependencies", {}).pop(name, None)
bundles = data.get("dsh", {}).get("profile", {}).get("bundles", [])
if name in bundles:
    bundles.remove(name)
with open(path, "w") as f:
    json.dump(data, f, ensure_ascii=False, indent=2)
    f.write("\n")
PY
  rm -rf "$PROFILE/node_modules/$PLUGIN_NAME" "$DSH_HOME/plugins/$PLUGIN_NAME"
  echo "已卸载 ${PLUGIN_NAME}（重启 DeepSeek 生效）。"
  exit 0
fi

[ -f "$BACKUP" ] || cp "$MANIFEST" "$BACKUP"
mkdir -p "$DSH_HOME/plugins" "$PROFILE/node_modules"
rm -rf "$DSH_HOME/plugins/$PLUGIN_NAME" "$PROFILE/node_modules/$PLUGIN_NAME"
cp -R "$SRC" "$DSH_HOME/plugins/$PLUGIN_NAME"
cp -R "$SRC" "$PROFILE/node_modules/$PLUGIN_NAME"

python3 - "$MANIFEST" "$PLUGIN_NAME" <<'PY'
import json, sys
path, name = sys.argv[1], sys.argv[2]
with open(path) as f:
    data = json.load(f)
changed = False
deps = data.setdefault("dependencies", {})
if name not in deps:
    deps[name] = "link:../../plugins/dsh-open-workspace"
    changed = True
profile = data.setdefault("dsh", {}).setdefault("profile", {})
bundles = profile.setdefault("bundles", [])
if name not in bundles:
    bundles.append(name)
    changed = True
if changed:
    with open(path, "w") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)
        f.write("\n")
PY

echo "已安装 ${PLUGIN_NAME}（重启 DeepSeek 生效）。"
