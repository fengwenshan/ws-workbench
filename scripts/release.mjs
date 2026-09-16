#!/usr/bin/env node
/**
 * 发布脚本：构建（含更新签名）→ 生成/合并 latest.json → 发布到 GitHub Release
 *
 * 用法：
 *   node scripts/release.mjs                 # 构建当前平台 + 发布
 *   node scripts/release.mjs --no-publish    # 只构建，产物落在 release/
 *   node scripts/release.mjs --target x86_64-pc-windows-msvc   # 交叉编译 Windows
 *
 * 两个平台分别在各自可用的环境里执行即可：脚本会把本平台条目「合并」进
 * 同一个 Release 的 latest.json，不会覆盖另一个平台。
 *
 * 需要的环境变量（发布时必填）：
 *   GITHUB_TOKEN     GitHub 个人访问令牌，需 repo（或 Contents: Read and write）权限
 *   GITHUB_REPO      默认 fengwenshan/project-manage
 *
 * 为什么用 GitHub 而不是内网 GitLab：
 *   内网 GitLab 项目无法设为公开，客户端拿不到任何文件；
 *   GitHub 公开仓库的 Release 资源可匿名下载，且 releases/latest/download 是
 *   Tauri 官方的标准更新地址。
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

const REPO = process.env.GITHUB_REPO || 'fengwenshan/project-manage'
const TOKEN = process.env.GITHUB_TOKEN || process.env.GH_TOKEN || ''
const SKIP_PUBLISH = process.argv.includes('--no-publish')

const KEY_PATH =
  process.env.TAURI_SIGNING_PRIVATE_KEY_PATH || join(process.env.HOME || '', '.tauri/demo-tools.key')

/** 更新清单的固定文件名 */
const ASSET_MANIFEST = 'latest.json'

/** 更新清单的公开地址（公开仓库，无需凭据） */
const MANIFEST_URL = `https://github.com/${REPO}/releases/latest/download/${ASSET_MANIFEST}`

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

function ghHeaders(extra = {}) {
  return {
    Authorization: `Bearer ${TOKEN}`,
    Accept: 'application/vnd.github+json',
    'X-GitHub-Api-Version': '2022-11-28',
    ...extra,
  }
}

async function gh(path, options = {}) {
  const res = await fetch(`https://api.github.com/repos/${REPO}${path}`, {
    ...options,
    headers: ghHeaders(options.headers),
  })
  if (!res.ok) {
    const text = await res.text()
    let hint = ''
    if (res.status === 401) {
      hint = '\n提示：GITHUB_TOKEN 无效或已过期，请重新生成。'
    } else if (res.status === 403) {
      hint =
        '\n提示：令牌权限不足。创建 Release 需要 repo 权限（细粒度令牌需 Contents: Read and write）。'
    } else if (res.status === 404) {
      hint = `\n提示：仓库 ${REPO} 不存在，或令牌无权访问。`
    }
    const err = new Error(`GitHub API ${res.status} ${path}: ${text.slice(0, 300)}${hint}`)
    err.status = res.status
    throw err
  }
  return res.status === 204 ? null : res.json()
}

/** 上传 Release 资源（走单独的 uploads 域名） */
async function uploadAsset(releaseId, filePath, fileName) {
  const res = await fetch(
    `https://uploads.github.com/repos/${REPO}/releases/${releaseId}/assets?name=${encodeURIComponent(fileName)}`,
    {
      method: 'POST',
      headers: ghHeaders({ 'Content-Type': 'application/octet-stream' }),
      body: readFileSync(filePath),
    }
  )
  if (!res.ok) {
    const text = await res.text()
    fail(`上传 ${fileName} 失败 (${res.status}): ${text.slice(0, 300)}`)
  }
  log(`已上传 ${fileName}`)
}

async function getReleaseByTag(tag) {
  const res = await fetch(`https://api.github.com/repos/${REPO}/releases/tags/${tag}`, {
    headers: ghHeaders(),
  })
  if (res.status === 404) return null
  if (!res.ok) fail(`读取 Release ${tag} 失败: ${res.status} ${(await res.text()).slice(0, 200)}`)
  return res.json()
}

/** 取回已发布的 latest.json，用于把另一个平台的条目合并进来 */
async function fetchPublishedManifest(release) {
  if (!release) return null
  try {
    const assets = await gh(`/releases/${release.id}/assets`)
    const m = assets.find((a) => a.name === ASSET_MANIFEST)
    if (!m) return null
    const res = await fetch(m.url, {
      headers: ghHeaders({ Accept: 'application/octet-stream' }),
    })
    if (!res.ok) return null
    return await res.json()
  } catch {
    return null
  }
}

/** 构建前的令牌校验，避免白等一次构建 */
function assertPublishToken() {
  if (SKIP_PUBLISH) return
  if (!TOKEN) {
    fail(
      '发布需要 GITHUB_TOKEN 环境变量。\n' +
        '  生成地址：https://github.com/settings/tokens\n' +
        '  经典令牌勾选 repo；细粒度令牌需 Contents: Read and write'
    )
  }
  if (TOKEN.startsWith('glpat-') || TOKEN.startsWith('gldt-')) {
    fail(
      '检测到 GitLab 令牌，但当前发布目标是 GitHub。\n' +
        `  目标仓库: ${REPO}\n` +
        '  请设置 GITHUB_TOKEN 为 GitHub 令牌。'
    )
  }
}

async function publish(version, platform, manifest) {
  const tag = `v${version}`
  let release = await getReleaseByTag(tag)

  if (!release) {
    log(`创建 Release ${tag} …`)
    release = await gh('/releases', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        tag_name: tag,
        name: tag,
        body: `版本 ${version}\n\n下载对应的安装包即可，客户端会自动检查更新。`,
        draft: false,
        prerelease: false,
      }),
    })
  } else {
    log(`Release ${tag} 已存在，替换本平台产物 …`)
  }

  // 同名资源需先删除，否则 GitHub 会拒绝重复上传
  const assets = await gh(`/releases/${release.id}/assets`)
  for (const a of assets) {
    if (a.name === ASSET_MANIFEST || a.name === platform.assetFile) {
      await gh(`/releases/assets/${a.id}`, { method: 'DELETE' })
      log(`已移除旧资源 ${a.name}`)
    }
  }

  await uploadAsset(release.id, join(OUT_DIR, platform.assetFile), platform.assetFile)
  await uploadAsset(release.id, join(OUT_DIR, ASSET_MANIFEST), ASSET_MANIFEST)

  log(`\x1b[32m发布完成\x1b[0m v${version} (${platform.key})`)
  log('本次包含平台: ' + Object.keys(manifest.platforms).join(', '))
  log('客户端更新地址（公开，无需凭据）:')
  log(`  ${MANIFEST_URL}`)
}

async function main() {
  const version = readVersion()
  const platform = resolvePlatform()

  log(`版本: ${version}`)
  log(`平台: ${platform.key} (${platform.label})`)
  log(`仓库: ${REPO}`)
  if (TARGET) log(`构建目标: ${TARGET}`)

  assertPublishToken()

  build()

  const { bundlePath, sigPath } = findArtifacts(platform)
  const signature = readFileSync(sigPath, 'utf8').trim()
  if (!signature) fail('签名文件为空，构建可能没有正确签名')

  mkdirSync(OUT_DIR, { recursive: true })
  copyFileSync(bundlePath, join(OUT_DIR, platform.assetFile))
  log(`已导出 release/${platform.assetFile}`)

  // 合并已发布的其他平台条目
  let platforms = {}
  if (TOKEN && !SKIP_PUBLISH) {
    const existing = await fetchPublishedManifest(await getReleaseByTag(`v${version}`))
    if (existing?.platforms) {
      platforms = { ...existing.platforms }
      log(`已读取到现有平台条目: ${Object.keys(platforms).join(', ')}`)
    }
  }

  platforms[platform.key] = {
    signature,
    url: `https://github.com/${REPO}/releases/latest/download/${platform.assetFile}`,
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
    log('已指定 --no-publish，跳过上传')
    return
  }
  await publish(version, platform, manifest)
}

main().catch((err) => fail(err.message))
