#!/usr/bin/env node
/**
 * 本地打包收尾：把刚打出来的 Windows 安装包复制到"工作区外面"的目录，
 * 并把校验和一起写出来。
 *
 * 为什么需要这一步：如果项目目录本身被运行沙箱/受限策略圈成"只能写本目录"，
 * 直接在 `src-tauri/target/release/bundle/nsis/` 里双击安装包，NSIS 解压到
 * `%TEMP%` 这一步就会被拒绝，静默模式会直接退出、没有任何提示；
 * 把安装包复制到下载/桌面等普通目录再运行就正常。
 *
 * 用法：
 *   node scripts/package-local.mjs
 *   node scripts/package-local.mjs --out D:\dist
 *   node scripts/package-local.mjs --open        # 复制完顺便打开所在文件夹
 *
 * 产物：
 *   <out>/ws-workbench_<version>_x64-setup.exe
 *   <out>/SHA256SUMS.txt
 */

import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const bundleDir = join(root, "src-tauri", "target", "release", "bundle", "nsis");

function parseArgs(argv) {
  const opts = { out: null, open: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--out" || arg === "-o") opts.out = argv[++i];
    else if (arg === "--open") opts.open = true;
    else if (arg === "--help" || arg === "-h") opts.help = true;
    else throw new Error(`未知参数: ${arg}`);
  }
  return opts;
}

function defaultOutDir() {
  return join(homedir(), "Downloads", "ws-workbench-build");
}

function isInside(parent, child) {
  const normalize = (value) => resolve(value).replace(/[\\/]+$/, "").toLowerCase();
  const p = normalize(parent);
  const c = normalize(child);
  return c === p || c.startsWith(p + "\\") || c.startsWith(p + "/");
}

function pickInstaller() {
  if (!existsSync(bundleDir)) {
    throw new Error(`没找到 ${bundleDir}，先跑一次构建：pnpm exec tauri build --bundles nsis`);
  }
  const candidates = readdirSync(bundleDir)
    .filter((name) => /^ws-workbench_.+_x64-setup\.exe$/i.test(name))
    .map((name) => ({ name, path: join(bundleDir, name), mtime: statSync(join(bundleDir, name)).mtimeMs }))
    .sort((a, b) => b.mtime - a.mtime);
  if (candidates.length === 0) {
    throw new Error(`${bundleDir} 里没有 ws-workbench_*_x64-setup.exe，先跑一次构建`);
  }
  return candidates[0];
}

function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex").toUpperCase();
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) {
    console.log(readFileSync(fileURLToPath(import.meta.url), "utf8").split("*/")[0]);
    return;
  }

  const installer = pickInstaller();
  const outDir = resolve(opts.out ?? defaultOutDir());
  if (isInside(root, outDir)) {
    throw new Error(`输出目录不能在工作区里（${outDir}）：放在工作区里的安装包，运行时可能被沙箱拦住。换一个普通目录，例如下载/桌面。`);
  }

  mkdirSync(outDir, { recursive: true });
  const target = join(outDir, installer.name);
  copyFileSync(installer.path, target);

  const hash = sha256(target);
  const size = statSync(target).size;
  writeFileSync(join(outDir, "SHA256SUMS.txt"), `${hash}  ${installer.name}\n`, "utf8");

  console.log("安装包已复制到工作区外：");
  console.log(`  ${target}`);
  console.log(`  大小   ${size} 字节`);
  console.log(`  SHA256 ${hash}`);
  console.log("");
  console.log("请从这个位置运行安装包（不要回到项目目录里双击）。");
  console.log("Windows 首次运行未签名安装包会弹 SmartScreen：更多信息 → 仍要运行。");

  if (opts.open) {
    try {
      execFileSync("explorer.exe", [outDir], { stdio: "ignore" });
    } catch {
      /* 打开失败不影响产物 */
    }
  }
}

main();
