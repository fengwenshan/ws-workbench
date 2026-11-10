# DSH 插件独立仓库发布 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `dsh-open-workspace` 插件从 ws-workbench 主仓导出成独立 git 仓库，按 tag 自动构建 zip 并发布到 GitHub Release。

**Architecture:** 用 `git subtree split --prefix=dsh-plugin/dsh-open-workspace` 在 ws-workbench 里生成插件自己的提交历史，克隆到 `/Users/fengwenshan/wenshan/demo/dsh-open-workspace` 作为独立仓库；加 GitHub Actions workflow（tag 触发：校验版本 → 打 zip → 建 Release）；本地先打 `v0.1.0` tag 并验证 zip 结构，推 GitHub 由用户执行（本机连不上 `github.com:443`）。

**Tech Stack:** git subtree、GitHub Actions、zip、Node（读 package.json）。

---

## 前置事实

- 插件源码现在在 ws-workbench：`dsh-plugin/dsh-open-workspace/`（`package.json`、`cordis.patch.yml`、`lib/index.mjs`、`lib/client.js`、`README.md`）。
- 目标目录 `/Users/fengwenshan/wenshan/demo/dsh-open-workspace` 已存在且为空。
- GitHub 仓库 `https://github.com/fengwenshan/dsh-open-workspace.git` 已由用户创建（当前空仓，default branch=main）。
- 本机 `github.com:443` 不可达（`git push` 会 75s 超时），但写文件、打 tag、`git log` 等本地操作正常；`api.github.com` 可访问，用于事后验证 Release。
- 团队约定：不提交用户工作区里未提交的改动；插件仓库在 workspace 之外，写它需要一次 `danger-full-access` 授权。

## 任务边界

- 本计划只动插件仓库（workspace 外），**不删** ws-workbench 里的 `dsh-plugin/`。
- ws-workbench 删除内置插件、改「设置 → 资源」的代码在另一个计划 `2026-11-10-ws-resource-manager.md`，且必须在任务 4 的 Release 可用之后执行。

## File Structure

```text
/Users/fengwenshan/wenshan/demo/dsh-open-workspace/   # 新的独立仓库（workspace 外）
  package.json
  cordis.patch.yml
  lib/index.mjs
  lib/client.js
  README.md
  LICENSE
  scripts/build-zip.sh          # 本地打 zip（CI 用同一条命令）
  .github/workflows/release.yml # tag 触发：校验版本 → zip → Release
```

---

### Task 1: 从主仓导出插件历史并初始化独立仓库

**Files:**
- Create: `/Users/fengwenshan/wenshan/demo/dsh-open-workspace/`（workspace 外）

- [ ] **Step 1: 在 ws-workbench 里生成插件历史分支**

Run:

```bash
cd /Users/fengwenshan/wenshan/demo/layui-admin-build
git subtree split --prefix=dsh-plugin/dsh-open-workspace -b dsh-open-workspace-split
```

Expected: 输出最新 commit hash；`git log --oneline dsh-open-workspace-split | head` 能看到之前提交插件代码的提交。

- [ ] **Step 2: 克隆该分支到目标目录**

Run:

```bash
git clone --branch dsh-open-workspace-split \
  /Users/fengwenshan/wenshan/demo/layui-admin-build \
  /Users/fengwenshan/wenshan/demo/dsh-open-workspace
cd /Users/fengwenshan/wenshan/demo/dsh-open-workspace
git checkout -b main
git branch -D dsh-open-workspace-split
git remote remove origin
git remote add origin https://github.com/fengwenshan/dsh-open-workspace.git
ls -la
```

Expected: 目录里有 `package.json`、`cordis.patch.yml`、`lib/`、`README.md`，没有 `.github/`。

- [ ] **Step 3: 提交空提交基线并打 v0.1.0 tag（先不打 zip）**

Run:

```bash
cd /Users/fengwenshan/wenshan/demo/dsh-open-workspace
node -p "require('./package.json').version"
git tag -a v0.1.0 -m "dsh-open-workspace v0.1.0"
git tag --list
```

Expected: `0.1.0`；`git tag --list` 有 `v0.1.0`。

- [ ] **Step 4: 回主仓删掉临时 split 分支（保留 tag 以外的干净状态）**

Run:

```bash
cd /Users/fengwenshan/wenshan/demo/layui-admin-build
git branch -D dsh-open-workspace-split
git branch --list "dsh-open-workspace-*"
```

Expected: 没有输出（分支已删）。

- [ ] **Step 5: 提交**

本任务在 workspace 外，不产生 ws-workbench 提交；若在 workspace 内产生临时分支已删除，确认 `git status --short` 与之前一致。

---

### Task 2: 加 release workflow、LICENSE、本地 zip 脚本

**Files:**
- Create: `/Users/fengwenshan/wenshan/demo/dsh-open-workspace/.github/workflows/release.yml`
- Create: `/Users/fengwenshan/wenshan/demo/dsh-open-workspace/scripts/build-zip.sh`
- Create: `/Users/fengwenshan/wenshan/demo/dsh-open-workspace/LICENSE`
- Modify: `/Users/fengwenshan/wenshan/demo/dsh-open-workspace/README.md`

- [ ] **Step 1: 写 release.yml**

Create `.github/workflows/release.yml`:

```yaml
name: release

on:
  push:
    tags:
      - "v*"

permissions:
  contents: write

jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: 校验 tag 与 package.json.version 一致
        run: |
          VERSION=$(node -p "require('./package.json').version")
          if [ "v$VERSION" != "$GITHUB_REF_NAME" ]; then
            echo "tag $GITHUB_REF_NAME 与 package.json version $VERSION 不一致" >&2
            exit 1
          fi
          echo "VERSION=$VERSION" >> "$GITHUB_ENV"

      - name: 打包 zip
        run: |
          zip -r "dsh-open-workspace-${VERSION}.zip" \
            package.json cordis.patch.yml lib README.md
          unzip -l "dsh-open-workspace-${VERSION}.zip"

      - name: 创建 GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          name: dsh-open-workspace ${{ github.ref_name }}
          files: dsh-open-workspace-*.zip
          generate_release_notes: true
          prerelease: false
```

- [ ] **Step 2: 写本地打包脚本**

Create `scripts/build-zip.sh`:

```bash
#!/usr/bin/env bash
# 本地按当前 package.json.version 打 zip；CI 的 release.yml 用同样规则。
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(node -p "require('./package.json').version")
OUT="dsh-open-workspace-${VERSION}.zip"
rm -f "$OUT"
zip -r "$OUT" package.json cordis.patch.yml lib README.md
echo "built $OUT"
unzip -l "$OUT"
```

Run: `chmod +x scripts/build-zip.sh`

- [ ] **Step 3: 加 LICENSE（MIT）**

Create `LICENSE`，内容为标准 MIT License，版权行：

```text
MIT License

Copyright (c) 2026 fengwenshan

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

- [ ] **Step 4: README 加「版本与发布」章节**

在 `README.md` 末尾追加：

```markdown
## 版本与发布

- 版本号就是 `package.json` 的 `version`；tag 必须写成 `v<version>`（如 `v0.1.0`）。
- 发布：`git tag -a v0.2.0 -m "v0.2.0" && git push origin main --tags`。
- GitHub Actions 在 tag push 后自动：
  1. 校验 tag 与 `package.json.version` 一致；
  2. 打 `dsh-open-workspace-<version>.zip`（根目录含 `package.json`、`cordis.patch.yml`、`lib/`、`README.md`）；
  3. 创建 Release 并上传 zip。
- ws-workbench 的「设置 → 资源」从 GitHub Release `latest` 读取版本并下载该 zip 安装。
- 本地打包：`scripts/build-zip.sh`（产物不入库，`.gitignore` 加 `*.zip`）。
```

- [ ] **Step 5: 加 .gitignore**

Create `.gitignore`:

```text
*.zip
node_modules/
.DS_Store
```

- [ ] **Step 6: 提交并补 tag**

Run:

```bash
cd /Users/fengwenshan/wenshan/demo/dsh-open-workspace
git add -A
git commit -m "chore: add release workflow, license and local zip script"
git tag -a v0.1.0 -m "dsh-open-workspace v0.1.0" --force
git log --oneline -3
git tag --list
```

Expected: 有 `v0.1.0` tag，工作树干净。

- [ ] **Step 7: 本地跑一次 build-zip 并检查结构**

Run:

```bash
cd /Users/fengwenshan/wenshan/demo/dsh-open-workspace
./scripts/build-zip.sh
unzip -l dsh-open-workspace-0.1.0.zip
```

Expected: zip 里条目都是根目录相对路径：

```text
package.json
cordis.patch.yml
lib/index.mjs
lib/client.js
README.md
```

（若 zip 出现 `dsh-open-workspace/` 前缀说明 `cd` 层级不对，回 Step 2 修脚本。）

---

### Task 3: 推送到 GitHub 并验证首个 Release（用户执行）

**Files:** 无

- [ ] **Step 1: 在某台能访问 github.com 的机器上推送**

Run（用户执行）：

```bash
cd /Users/fengwenshan/wenshan/demo/dsh-open-workspace
git push -u origin main
git push origin v0.1.0
```

Expected: 推送成功，GitHub Actions 开始跑 `release` workflow。

- [ ] **Step 2: 等 workflow 完成并验证 Release**

Run（本机可执行，`api.github.com` 可达）：

```bash
curl -sS -m 10 https://api.github.com/repos/fengwenshan/dsh-open-workspace/releases/latest \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['tag_name']); print([a['name'] for a in d['assets']])"
```

Expected: `v0.1.0` 与 `['dsh-open-workspace-0.1.0.zip']`。

- [ ] **Step 3: 验证 zip 资产可下载（ws-workbench 用的正是这条路径）**

Run:

```bash
ASSET=$(curl -sS -m 10 https://api.github.com/repos/fengwenshan/dsh-open-workspace/releases/latest | python3 -c "import json,sys; print(json.load(sys.stdin)['assets'][0]['url'])")
curl -sS -m 20 -L -H "Accept: application/octet-stream" -o /tmp/dsh-open-workspace-0.1.0.zip "$ASSET"
unzip -l /tmp/dsh-open-workspace-0.1.0.zip
rm -f /tmp/dsh-open-workspace-0.1.0.zip
```

Expected: 下载成功、结构同 Task 2 Step 7。

- [ ] **Step 4: 更新 plan/README 里的状态（可选）**

在 ws-workbench 的 `docs/superpowers/plans/2026-11-10-ws-resource-manager.md` 顶部「前置」里把
「Release 可用」打勾，或在提交信息里注明 `dsh-open-workspace v0.1.0 已发布`。

---

## 风险与回退

- `github.com:443` 不通时 push 会卡 75 秒后失败：不要在自动化里跑 push，交给用户手动执行。
- tag 与版本不一致会让 workflow 失败：发布前先跑 `node -p "require('./package.json').version"` 核对。
- Linux 上 `zip` 可能未安装：workflow 的 `ubuntu-latest` 自带 zip；本地脚本报 `command not found` 时用 `brew install zip`。
- 回退：删掉远端 Release/tag 即可，ws-workbench 的版本检查会退回到上一个 latest。
