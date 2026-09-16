#!/usr/bin/env node
/**
 * 发布脚本：构建（含更新签名）→ 生成/合并 latest.json → 上传到 GitLab Release
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
 *   GITLAB_TOKEN     个人访问令牌，需 api 权限
 *   GITLAB_HOST      默认 gitlab.example.com
 *   GITLAB_PROJECT   默认 example-group/demo-tools
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

const HOST = process.env.GITLAB_HOST || 'gitlab.example.com'
const PROJECT = process.env.GITLAB_PROJECT || 'example-group/demo-tools'
const PROJECT_ID = encodeURIComponent(PROJECT)
const TOKEN = process.env.GITLAB_TOKEN || ''
const SKIP_PUBLISH = process.argv.includes('--no-publish')

const KEY_PATH =
  process.env.TAURI_SIGNING_PRIVATE_KEY_PATH || join(process.env.HOME || '', '.tauri/demo-tools.key')

/** latest.json 在 Release 里的固定文件名 */
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
  // 在非 Windows 主机上交叉编译 Windows：需要 cargo-xwin + nsis + llvm
  if (TARGET.includes('windows') && process.platform !== 'win32') {
    args.push('--runner', 'cargo-xwin')
  }

  log(`使用签名私钥: ${KEY_PATH}`)
  log(`开始构建: pnpm ${args.join(' ')}`)
  // 打包器只识别 TAURI_SIGNING_PRIVATE_KEY（内容或路径），
  // TAURI_SIGNING_PRIVATE_KEY_PATH 不生效，实测会报 "no private key"。
  const privateKey = readFileSync(KEY_PATH, 'utf8')
  execFileSync('pnpm', args, {
    cwd: ROOT,
    stdio: 'inherit',
    env: { ...process.env, TAURI_SIGNING_PRIVATE_KEY: privateKey },
  })
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

async function api(path, options = {}) {
  const res = await fetch(`https://${HOST}/api/v4/projects/${PROJECT_ID}${path}`, {
    ...options,
    headers: { 'PRIVATE-TOKEN': TOKEN, ...(options.headers || {}) },
  })
  if (!res.ok) {
    const text = await res.text()
    const err = new Error(`GitLab API ${res.status} ${path}: ${text.slice(0, 400)}`)
    err.status = res.status
    throw err
  }
  return res.json()
}

async function uploadFile(filePath, fileName) {
  const form = new FormData()
  form.append('file', new Blob([readFileSync(filePath)]), fileName)
  const data = await api('/uploads', { method: 'POST', body: form })
  log(`已上传 ${fileName}`)
  return `https://${HOST}${data.full_path}`
}

async function getRelease(tag) {
  const res = await fetch(
    `https://${HOST}/api/v4/projects/${PROJECT_ID}/releases/${encodeURIComponent(tag)}`,
    { headers: { 'PRIVATE-TOKEN': TOKEN } }
  )
  if (res.status === 404) return null
  if (!res.ok) fail(`读取 Release ${tag} 失败: ${res.status} ${(await res.text()).slice(0, 300)}`)
  return res.json()
}

/** 取回已发布的 latest.json，用于把另一个平台的条目合并进来 */
async function fetchPublishedManifest(release) {
  const link = (release?.assets?.links || []).find((l) => l.name === ASSET_MANIFEST)
  if (!link) return null
  try {
    const res = await fetch(link.url)
    if (!res.ok) return null
    return await res.json()
  } catch {
    return null
  }
}

async function upsertLink(tag, release, link) {
  const existing = (release.assets.links || []).find((l) => l.name === link.name)
  const body = {
    name: link.name,
    url: link.url,
    direct_asset_path: link.filepath,
    link_type: link.link_type,
  }
  if (existing) {
    await api(`/releases/${encodeURIComponent(tag)}/assets/links/${existing.id}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
    log(`已更新资源链接 ${link.name}`)
  } else {
    await api(`/releases/${encodeURIComponent(tag)}/assets/links`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
    log(`已新增资源链接 ${link.name}`)
  }
}

async function publish(version, platform, manifest) {
  if (!TOKEN) {
    fail('发布需要 GITLAB_TOKEN 环境变量（个人访问令牌，勾选 api 权限）')
  }
  const tag = `v${version}`

  const bundleUrl = await uploadFile(join(OUT_DIR, platform.assetFile), platform.assetFile)
  const manifestUrl = await uploadFile(join(OUT_DIR, ASSET_MANIFEST), ASSET_MANIFEST)

  const release = await getRelease(tag)
  const links = [
    { name: ASSET_MANIFEST, url: manifestUrl, filepath: `/${ASSET_MANIFEST}`, link_type: 'other' },
    {
      name: platform.assetFile,
      url: bundleUrl,
      filepath: `/${platform.assetFile}`,
      link_type: 'other',
    },
  ]

  if (!release) {
    log(`创建 Release ${tag} …`)
    await api('/releases', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name: tag,
        tag_name: tag,
        description: `版本 ${version}\n\n安装包与更新清单见下方附件。\n\n当前已包含平台：${platform.key}`,
        assets: { links },
      }),
    })
  } else {
    log(`Release ${tag} 已存在，合并 ${platform.key} 条目 …`)
    for (const link of links) {
      await upsertLink(tag, release, link)
    }
  }

  log(`\x1b[32m发布完成\x1b[0m v${version} (${platform.key})`)
  log('已包含平台: ' + Object.keys(manifest.platforms).join(', '))
  log(`更新清单: https://${HOST}/${PROJECT}/-/releases/permalink/latest/downloads/${ASSET_MANIFEST}`)
}

async function main() {
  const version = readVersion()
  const platform = resolvePlatform()

  log(`版本: ${version}`)
  log(`平台: ${platform.key} (${platform.label})`)
  if (TARGET) log(`构建目标: ${TARGET}`)

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
    const existing = await fetchPublishedManifest(await getRelease(`v${version}`))
    if (existing?.platforms) {
      platforms = { ...existing.platforms }
      log(`已读取到现有平台条目: ${Object.keys(platforms).join(', ')}`)
    }
  }

  platforms[platform.key] = {
    signature,
    url: `https://${HOST}/${PROJECT}/-/releases/permalink/latest/downloads/${platform.assetFile}`,
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
