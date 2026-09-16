#!/usr/bin/env node
/**
 * 发布脚本：构建（含更新签名）→ 生成/合并 latest.json → 提交到 Gitee updates/ 目录
 *
 * 用法：
 *   node scripts/release.mjs                 # 构建当前平台 + 发布
 *   node scripts/release.mjs --no-publish    # 只构建，产物落在 release/
 *   node scripts/release.mjs --target x86_64-pc-windows-msvc   # 交叉编译 Windows
 *
 * 两个平台分别在各自可用的环境里执行即可：脚本会把本平台条目「合并」进
 * updates/latest.json，不会覆盖另一个平台。
 *
 * 发布不需要任何 API 令牌：产物通过 git push 提交，客户端走 Gitee raw 地址拉取。
 *
 * 为什么不用 GitLab / GitHub：
 *   内网 GitLab 项目无法设为公开，客户端拿不到文件；
 *   GitHub 的 github.com 与 raw.githubusercontent.com 在你们网络下被拦（实测超时），
 *   Gitee 的 raw 地址实测匿名可读。
 */
import { execFileSync } from 'node:child_process'
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  copyFileSync,
  writeFileSync,
} from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const TAURI_DIR = join(ROOT, 'src-tauri')
const OUT_DIR = join(ROOT, 'release')
/** 更新产物在仓库里的目录（会被提交，客户端通过 raw 地址读取） */
const UPDATES_DIR = 'updates'

const REPO = process.env.GITEE_REPO || 'feng_wenshan/project-manage'
const BRANCH = process.env.GITEE_BRANCH || 'main'
const SKIP_PUBLISH = process.argv.includes('--no-publish')

/** 客户端读取更新文件的地址前缀（公开仓库，无需凭据） */
const RAW_BASE = `https://gitee.com/${REPO}/raw/${BRANCH}/${UPDATES_DIR}`

const KEY_PATH =
  process.env.TAURI_SIGNING_PRIVATE_KEY_PATH || join(process.env.HOME || '', '.tauri/demo-tools.key')

/** 更新清单的固定文件名 */
const ASSET_MANIFEST = 'latest.json'

function argValue(flag) {
  const i = process.argv.indexOf(flag)
  return i >= 0 ? process.argv[i + 1] : ''
}

const TARGET = process.env.TAURI_TARGET || argValue('--target') || ''

function log(msg) {
  console.log(`\x1b[36m[release]\x1b[0m ${msg}`)
}
function fail(msg) {
  console.error(`\x1b[31m[release] 失败: ${msg}\x1b[0m`)
  process.exit(1)
}

/** 当前构建目标的平台信息 */
function resolvePlatform() {
  const isWindowsTarget = TARGET.includes('windows') || process.platform === 'win32'
  const archRaw = TARGET
    ? TARGET.split('-')[0] // x86_64 / aarch64 / i686
    : process.arch === 'arm64'
      ? 'aarch64'
      : process.arch

  if (isWindowsTarget) {
    const arch = archRaw === 'arm64' ? 'aarch64' : archRaw
    return {
      key: `windows-${arch}`,
      bundleSubdir: 'nsis',
      // 优先 NSIS 的 -setup.exe，其次 MSI
      matchBundle: (f) => f.endsWith('-setup.exe') || f.endsWith('.msi'),
      matchSig: (f) => f.endsWith('-setup.exe.sig') || f.endsWith('.msi.sig'),
      assetFile: 'demo-tools-setup.exe',
      label: 'Windows',
    }
  }
  if (process.platform === 'darwin' || TARGET.includes('apple')) {
    const arch = archRaw === 'x86_64' ? 'x86_64' : 'aarch64'
    return {
      key: `darwin-${arch}`,
      bundleSubdir: 'macos',
      matchBundle: (f) => f.endsWith('.app.tar.gz'),
      matchSig: (f) => f.endsWith('.app.tar.gz.sig'),
      assetFile: 'demo-tools.app.tar.gz',
      label: 'macOS',
    }
  }
  return fail(`不支持的构建平台: target=${TARGET || '(host)'} platform=${process.platform}`)
}

function bundleDir(platform) {
  return TARGET
    ? join(TAURI_DIR, 'target', TARGET, 'release/bundle', platform.bundleSubdir)
    : join(TAURI_DIR, 'target/release/bundle', platform.bundleSubdir)
}

function readVersion() {
  const conf = JSON.parse(readFileSync(join(TAURI_DIR, 'tauri.conf.json'), 'utf8'))
  if (!conf.version) fail('tauri.conf.json 里没有 version 字段')
  return conf.version
}

function build() {
  if (!existsSync(KEY_PATH)) {
    fail(
      `找不到签名私钥: ${KEY_PATH}\n` +
        '先执行: npx tauri signer generate -w ~/.tauri/demo-tools.key'
    )
  }
  const args = ['tauri', 'build']
  if (TARGET) args.push('--target', TARGET)
  // 可用 TAURI_BUNDLES=app 跳过 dmg（例如受限环境无法创建 dmg 时）；
  // 更新通道只需要 .app.tar.gz，不依赖 dmg。
  if (process.env.TAURI_BUNDLES) args.push('--bundles', process.env.TAURI_BUNDLES)
  // 在非 Windows 主机上交叉编译 Windows：需要 cargo-xwin + nsis + llvm
  if (TARGET.includes('windows') && process.platform !== 'win32') {
    args.push('--runner', 'cargo-xwin')
  }

  log(`使用签名私钥: ${KEY_PATH}`)
  log(`开始构建: pnpm ${args.join(' ')}`)

  const env = {
    ...process.env,
    // 打包器只识别 TAURI_SIGNING_PRIVATE_KEY（内容或路径），
    // TAURI_SIGNING_PRIVATE_KEY_PATH 不生效，实测会报 "no private key"。
    TAURI_SIGNING_PRIVATE_KEY: readFileSync(KEY_PATH, 'utf8'),
  }

  // 交叉编译 Windows 需要 llvm-rc（Tauri 用它编译资源文件）和 makensis。
  // Homebrew 的 llvm 是 keg-only，不在默认 PATH 里，这里自动补上，避免每次手动 export。
  if (TARGET.includes('windows') && process.platform !== 'win32') {
    const extra = [
      '/opt/homebrew/opt/llvm/bin',
      '/usr/local/opt/llvm/bin',
      '/opt/homebrew/bin',
      '/usr/local/bin',
    ].filter(existsSync)
    env.PATH = [...extra, env.PATH].join(':')
    log(`已补充 PATH: ${extra.join(', ')}`)
  }

  execFileSync('pnpm', args, { cwd: ROOT, stdio: 'inherit', env })
}

function findArtifacts(platform) {
  const dir = bundleDir(platform)
  if (!existsSync(dir)) {
    fail(`找不到构建产物目录: ${dir}\n请确认构建已成功完成`)
  }
  const files = readdirSync(dir)
  const bundle = files.find(platform.matchBundle)
  const sig = files.find(platform.matchSig)
  if (!bundle || !sig) {
    fail(
      `在 ${dir} 里找不到安装包或签名文件\n` +
        `实际文件: ${files.join(', ')}\n` +
        '请确认 tauri.conf.json 的 bundle.createUpdaterArtifacts 为 true'
    )
  }
  return { bundlePath: join(dir, bundle), sigPath: join(dir, sig) }
}

/** 读取已提交的 latest.json，用于把另一个平台的条目合并进来 */
function readPublishedManifest() {
  const p = join(ROOT, UPDATES_DIR, ASSET_MANIFEST)
  if (!existsSync(p)) return null
  try {
    return JSON.parse(readFileSync(p, 'utf8'))
  } catch {
    return null
  }
}

function git(args, opts = {}) {
  return execFileSync('git', args, { cwd: ROOT, encoding: 'utf8', ...opts })
}

/** 发布：把产物写进 updates/ 并推送到远端（无需任何 Token） */
function publish(version, platform, manifest) {
  const dest = join(ROOT, UPDATES_DIR)
  mkdirSync(dest, { recursive: true })
  copyFileSync(join(OUT_DIR, platform.assetFile), join(dest, platform.assetFile))
  copyFileSync(join(OUT_DIR, ASSET_MANIFEST), join(dest, ASSET_MANIFEST))
  log(`已写入 ${UPDATES_DIR}/`)

  git(['add', UPDATES_DIR], { stdio: 'inherit' })

  const staged = git(['diff', '--cached', '--name-only']).trim()
  if (!staged) {
    log('updates/ 内容无变化，跳过提交')
    return
  }

  git(
    ['commit', '-m', `release: v${version} (${platform.key})`, '--', UPDATES_DIR],
    { stdio: 'inherit' }
  )
  log('推送到远端 …')
  git(['push', 'origin', `HEAD:${BRANCH}`], { stdio: 'inherit' })

  log(`\x1b[32m发布完成\x1b[0m v${version} (${platform.key})`)
  log('本次包含平台: ' + Object.keys(manifest.platforms).join(', '))
  log('客户端更新地址（公开，无需凭据）:')
  log(`  ${RAW_BASE}/${ASSET_MANIFEST}`)
}

async function main() {
  const version = readVersion()
  const platform = resolvePlatform()

  log(`版本: ${version}`)
  log(`平台: ${platform.key} (${platform.label})`)
  log(`仓库: ${REPO}  分支: ${BRANCH}`)
  if (TARGET) log(`构建目标: ${TARGET}`)

  build()

  const { bundlePath, sigPath } = findArtifacts(platform)
  const signature = readFileSync(sigPath, 'utf8').trim()
  if (!signature) fail('签名文件为空，构建可能没有正确签名')

  mkdirSync(OUT_DIR, { recursive: true })
  copyFileSync(bundlePath, join(OUT_DIR, platform.assetFile))
  log(`已导出 release/${platform.assetFile}`)

  // 合并已提交的其他平台条目
  const published = readPublishedManifest()
  const platforms = published?.platforms ? { ...published.platforms } : {}
  if (published?.platforms) {
    log(`已读取到现有平台条目: ${Object.keys(platforms).join(', ')}`)
  }

  platforms[platform.key] = {
    signature,
    url: `${RAW_BASE}/${platform.assetFile}`,
  }

  const manifest = {
    version,
    notes: `OA 部署打包工具 v${version}`,
    pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
    platforms,
  }
  writeFileSync(join(OUT_DIR, ASSET_MANIFEST), JSON.stringify(manifest, null, 2) + '\n')
  log(`已生成 release/${ASSET_MANIFEST}（平台: ${Object.keys(platforms).join(', ')}）`)

  if (SKIP_PUBLISH) {
    log('已指定 --no-publish，跳过提交')
    return
  }
  publish(version, platform, manifest)
}

main().catch((err) => fail(err.message))
