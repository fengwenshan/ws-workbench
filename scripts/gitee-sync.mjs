#!/usr/bin/env node
/**
 * Gitee 镜像同步：把这次发版的三个平台更新包（自动更新用）和手工安装包
 * （dmg / msi）镜像到 Gitee，并把更新清单里的下载地址改成 Gitee。客户端
 * 检查更新时先读 Gitee 清单，读不到（超时 / 404）再读 GitHub 清单兜底。
 *
 * 为什么单开一个脚本，而不是塞进 release.mjs：
 *   三个平台的构建 job 是并行的、各自只合并自己那条清单；镜像必须等三份安装包
 *   都齐了才有意义，所以放在全部构建成功之后串行跑一次。Gitee 侧只有一个写者，
 *   不用处理并发写清单。
 *
 * Gitee 侧结构（和 GitHub 一一对应）：
 *   - 每次发版建一个 tag `vX.Y.Z` + 同 tag 的发行版，附件名与 GitHub 完全相同
 *     （ws-workbench-setup.exe / ws-workbench_<arch>.app.tar.gz 这些更新载荷，
 *     外加 ws-workbench_<ver>_<arch>.dmg / ws-workbench_<ver>_x64_en-US.msi
 *     这些手工安装包），旧版本的下载地址不会失效。
 *   - 清单文件：<默认分支>/updates/latest.json，客户端从 raw 地址读取。
 *   - 全程只用 Gitee API（私人令牌），不在 Gitee 上推 git。
 *
 * 顺序上先写一份「下载地址还是 GitHub」的清单到 Gitee，再上传附件、校验、
 * 最后把地址换成 Gitee：即使上传失败，Gitee 清单里的版本号也是最新的，
 * 不会出现「Gitee 清单停在上一版 → 新客户端以为没有更新」。
 *
 * 用法（CI 里由 release.yml 的 gitee-sync job 调用）：
 *   GITEE_TOKEN=xxx node scripts/gitee-sync.mjs --version 1.0.22 --dir release-assets
 *   GITEE_TOKEN=xxx node scripts/gitee-sync.mjs --version 1.0.22 --dir release --dry-run
 *
 * 环境变量：
 *   GITEE_TOKEN   Gitee 私人令牌（仓库读写权限）；CI 来自 secrets.GITEE_TOKEN
 *   GITEE_REPO    Gitee 仓库，默认 feng_wenshan/ws-workbench（必须是公开仓库）
 *   GITEE_BRANCH  清单所在分支；不设时用仓库的默认分支（Gitee 新建仓库默认 master）
 *   GITHUB_REPO   GitHub 仓库，默认 fengwenshan/ws-workbench
 *   GITHUB_BRANCH GitHub 清单分支，默认 main
 */
import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs'
import { request as httpRequest } from 'node:http'
import { request as httpsRequest } from 'node:https'
import { randomBytes } from 'node:crypto'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const UPDATES_DIR = 'updates'
const ASSET_MANIFEST = 'latest.json'
const UA = 'ws-workbench-release-sync'

const GITEE_REPO = process.env.GITEE_REPO || 'feng_wenshan/ws-workbench'
const GITEE_BRANCH_ENV = process.env.GITEE_BRANCH || ''
const GITEE_TOKEN = process.env.GITEE_TOKEN || ''
const GITEE_API = process.env.GITEE_API_BASE || 'https://gitee.com/api/v5'
/** Gitee 网站根地址（raw / releases/download 用），测试时可以指向本地 mock */
const GITEE_WEB = process.env.GITEE_WEB_BASE || 'https://gitee.com'

const GITHUB_BRANCH = process.env.GITHUB_BRANCH || 'main'

/**
 * 更新载荷：本地文件名（与 release.mjs 的 assetFile 一致）→ 更新清单里的平台键。
 * 上传到 Gitee 时沿用同名文件，清单里的 URL 按 `releases/download/<tag>/<文件名>` 拼。
 */
const PLATFORMS = [
  { key: 'windows-x86_64', asset: 'ws-workbench-setup.exe' },
  { key: 'darwin-aarch64', asset: 'ws-workbench_aarch64.app.tar.gz' },
  { key: 'darwin-x86_64', asset: 'ws-workbench_x86_64.app.tar.gz' },
]

/**
 * 手工安装包：自动更新用不到 dmg / msi（macOS 走 .app.tar.gz，Windows 走 NSIS
 * exe），传一份到 Gitee 只是方便国内用户手动下载安装。文件名里带版本号，所以
 * 按清单版本拼名字去找；哪份没构建出来就跳过哪份，不影响发版和更新清单。
 */
const MANUAL_INSTALLERS = [
  { label: 'macOS ARM64 dmg', asset: (version) => `ws-workbench_${version}_aarch64.dmg` },
  { label: 'macOS Intel dmg', asset: (version) => `ws-workbench_${version}_x86_64.dmg` },
  { label: 'Windows x64 msi', asset: (version) => `ws-workbench_${version}_x64_en-US.msi` },
]

function argValue(flag) {
  const i = process.argv.indexOf(flag)
  return i >= 0 ? process.argv[i + 1] || '' : ''
}

const VERSION = argValue('--version') || process.env.RELEASE_VERSION || ''
const DIR = resolve(ROOT, argValue('--dir') || 'release-assets')
const MANIFEST_FROM = argValue('--manifest') || ''
const DRY_RUN = process.argv.includes('--dry-run')
const NO_GITHUB = process.argv.includes('--no-github')
/**
 * 补镜像时用：发行版上已有同名且大小一致的附件就跳过上传。
 * 发版流程不用它，发版要保证 Gitee 上就是这次构建的字节。
 */
const SKIP_EXISTING = process.argv.includes('--skip-existing')

function log(msg) {
  console.log(`\x1b[36m[gitee-sync]\x1b[0m ${msg}`)
}
function warn(msg) {
  console.warn(`\x1b[33m[gitee-sync]\x1b[0m ${msg}`)
}
function fail(msg) {
  console.error(`\x1b[31m[gitee-sync] 失败: ${msg}\x1b[0m`)
  process.exit(1)
}
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

/** 递归列出目录下的文件：CI 里 download-artifact 会按 artifact 名建一层子目录。 */
function walk(dir) {
  const out = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) out.push(...walk(path))
    else if (entry.isFile()) out.push(path)
  }
  return out
}

function git(args) {
  return execFileSync('git', args, {
    cwd: ROOT,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, GIT_TERMINAL_PROMPT: '0' },
  })
}

/** 拉最新 main：显式 refspec，避免 CI 里 checkout 出来的远端引用不完整。 */
function fetchBranch(branch) {
  git(['fetch', 'origin', `+refs/heads/${branch}:refs/remotes/origin/${branch}`])
}

/** 找到本次要镜像的更新载荷和手工安装包；缺哪份就跳过哪份（载荷缺了清单里保留 GitHub 地址）。 */
function locateAssets(version) {
  if (!existsSync(DIR)) fail(`产物目录不存在: ${DIR}（CI 里由 release.yml 的 download-artifact 准备）`)
  const files = walk(DIR)
  const found = {}
  const missing = []
  for (const platform of PLATFORMS) {
    const hit = files.find((path) => basename(path) === platform.asset)
    if (hit) found[platform.key] = hit
    else missing.push(`${platform.asset}（${platform.key}）`)
  }
  if (missing.length) warn(`缺少构建产物，对应平台保持 GitHub 地址: ${missing.join('、')}`)

  const installers = []
  const missingInstallers = []
  for (const installer of MANUAL_INSTALLERS) {
    const name = installer.asset(version)
    const hit = files.find((path) => basename(path) === name)
    if (hit) installers.push({ name, path: hit, label: installer.label })
    else missingInstallers.push(name)
  }
  if (missingInstallers.length) {
    warn(`缺少手工安装包（不影响更新，Gitee 上不提供这份）: ${missingInstallers.join('、')}`)
  }
  return { found, installers }
}

/**
 * 读这次发版的基准清单：优先取远端 main（三个构建 job 刚合并好的那份），
 * 本地跑时可以用 --manifest 指定文件。
 */
function readBaseManifest() {
  if (MANIFEST_FROM) {
    const path = resolve(ROOT, MANIFEST_FROM)
    log(`清单来源: ${path}`)
    return JSON.parse(readFileSync(path, 'utf8'))
  }
  try {
    fetchBranch(GITHUB_BRANCH)
    const raw = git(['show', `origin/${GITHUB_BRANCH}:${UPDATES_DIR}/${ASSET_MANIFEST}`])
    log(`清单来源: origin/${GITHUB_BRANCH}:${UPDATES_DIR}/${ASSET_MANIFEST}`)
    return JSON.parse(raw)
  } catch (err) {
    const local = join(ROOT, UPDATES_DIR, ASSET_MANIFEST)
    if (!existsSync(local)) fail('既读不到远端清单，也没有本地 updates/latest.json')
    warn(`读 origin/${GITHUB_BRANCH} 清单失败，退回本地 ${local}`)
    return JSON.parse(readFileSync(local, 'utf8'))
  }
}

/** 调 Gitee Open API：参数走 form（Gitee 的写接口都是 formData），token 同时放 query 兜底。 */
async function gitee(path, { method = 'GET', form, query } = {}) {
  const url = new URL(GITEE_API + path)
  if (GITEE_TOKEN) url.searchParams.set('access_token', GITEE_TOKEN)
  if (query) {
    for (const [key, value] of Object.entries(query)) {
      if (value !== undefined && value !== null) url.searchParams.set(key, String(value))
    }
  }

  const init = { method, headers: { 'User-Agent': UA } }
  if (form) {
    const params = new URLSearchParams()
    for (const [key, value] of Object.entries(form)) {
      if (value !== undefined && value !== null) params.set(key, String(value))
    }
    init.headers['Content-Type'] = 'application/x-www-form-urlencoded'
    init.body = params.toString()
  }

  const res = await fetch(url, init)
  const text = await res.text()
  let json = null
  try {
    json = text ? JSON.parse(text) : null
  } catch {
    json = null
  }
  if (!res.ok) {
    const err = new Error(`Gitee API ${method} ${path} 返回 HTTP ${res.status}: ${text.slice(0, 400)}`)
    err.status = res.status
    throw err
  }
  return json
}

async function withRetry(fn, label, attempts = 3) {
  let last
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      return await fn()
    } catch (err) {
      last = err
      warn(`${label} 第 ${attempt}/${attempts} 次失败: ${err.message}`)
      if (attempt < attempts) await sleep(3000 * attempt)
    }
  }
  throw last
}

/** 解析出清单分支：优先环境变量，其次仓库默认分支（新建 Gitee 仓库默认 master）。 */
async function resolveBranch() {
  if (GITEE_BRANCH_ENV) {
    log(`清单分支: ${GITEE_BRANCH_ENV}（来自 GITEE_BRANCH）`)
    return GITEE_BRANCH_ENV
  }
  try {
    const repo = await gitee(`/repos/${GITEE_REPO}`)
    const branch = repo.default_branch || 'master'
    log(`清单分支: ${branch}（仓库默认分支）`)
    return branch
  } catch (err) {
    fail(`读不到 Gitee 仓库 ${GITEE_REPO}：${err.message}\n请确认仓库已创建、并且是公开仓库。`)
  }
}

function downloadBase() {
  return `${GITEE_WEB}/${GITEE_REPO}/releases/download`
}
function manifestUrl(branch) {
  return `${GITEE_WEB}/${GITEE_REPO}/raw/${branch}/${UPDATES_DIR}/${ASSET_MANIFEST}`
}

async function listAttachments(release) {
  const list = await gitee(`/repos/${GITEE_REPO}/releases/${release.id}/attach_files`)
  return Array.isArray(list) ? list : []
}

/**
 * 上传一个附件（multipart/form-data），用 Node 内置 http(s) 直连，不设超时。
 *
 * 为什么不用 fetch：Node 内置 fetch（undici）默认 headersTimeout 是 300s，
 * 而 Gitee 要等 GitHub 海外 runner 把 15MB+ 的整个包传完才回响应头；一旦上传
 * 超过 5 分钟，fetch 就报 `fetch failed`（UND_ERR_HEADERS_TIMEOUT），重试也
 * 一样。Gitee 上 exe / tar.gz 能传上去、dmg / msi 三个全部失败就是这么来的。
 * 这里不设超时，慢就慢，至少能传完。
 */
function uploadMultipart(urlString, fields, fileField) {
  return new Promise((resolvePromise, rejectPromise) => {
    const url = new URL(urlString)
    const boundary = `----wsbench${randomBytes(12).toString('hex')}`
    const parts = []
    for (const [name, value] of Object.entries(fields)) {
      parts.push(Buffer.from(`--${boundary}\r\nContent-Disposition: form-data; name="${name}"\r\n\r\n${value}\r\n`))
    }
    parts.push(
      Buffer.from(
        `--${boundary}\r\n` +
          `Content-Disposition: form-data; name="${fileField.name}"; filename="${fileField.filename}"\r\n` +
          'Content-Type: application/octet-stream\r\n\r\n'
      )
    )
    parts.push(fileField.bytes)
    parts.push(Buffer.from(`\r\n--${boundary}--\r\n`))
    const body = Buffer.concat(parts)

    const request = (url.protocol === 'http:' ? httpRequest : httpsRequest)(
      url,
      {
        method: 'POST',
        headers: {
          'User-Agent': UA,
          'Content-Type': `multipart/form-data; boundary=${boundary}`,
          'Content-Length': String(body.length),
        },
      },
      (res) => {
        const chunks = []
        res.on('data', (chunk) => chunks.push(chunk))
        res.on('end', () =>
          resolvePromise({ status: res.statusCode, text: Buffer.concat(chunks).toString('utf8') })
        )
      }
    )
    request.on('error', rejectPromise)
    request.end(body)
  })
}

/** 上传一个附件；重跑同一版本时先删掉同名旧附件，保证字节是这次构建的。 */
async function uploadAttachment(release, localPath, remoteName) {
  const bytes = readFileSync(localPath)
  const size = bytes.length
  const url = new URL(`${GITEE_API}/repos/${GITEE_REPO}/releases/${release.id}/attach_files`)
  if (GITEE_TOKEN) url.searchParams.set('access_token', GITEE_TOKEN)

  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      const existing = (await listAttachments(release)).find((item) => item.name === remoteName)
      if (existing) {
        if (SKIP_EXISTING && Number(existing.size) === size) {
          log(`附件 ${remoteName} 已存在且大小一致（${size} 字节），跳过上传`)
          return
        }
        log(`附件 ${remoteName} 已存在（id=${existing.id}），先删除再重传`)
        await gitee(`/repos/${GITEE_REPO}/releases/${release.id}/attach_files/${existing.id}`, {
          method: 'DELETE',
        }).catch((err) => warn(`删除旧附件失败（继续上传）: ${err.message}`))
      }

      const res = await uploadMultipart(url.toString(), {}, { name: 'file', filename: remoteName, bytes })
      if (res.status < 200 || res.status >= 300) {
        throw new Error(`HTTP ${res.status}: ${res.text.slice(0, 300)}`)
      }
      log(`已上传 ${remoteName}（${(size / 1048576).toFixed(1)}MB）`)
      return
    } catch (err) {
      if (attempt === 3) throw err
      const code = err.cause?.code || err.code || ''
      warn(`上传 ${remoteName} 第 ${attempt} 次失败，${3 * attempt}s 后重试: ${err.message}${code ? `（${code}）` : ''}`)
      await sleep(3000 * attempt)
    }
  }
}

/** 校验 Gitee 下载地址真的能拿到文件，并且大小和本地一致（Gitee 会 302 到 CDN）。 */
async function verifyRemote(url, size) {
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      const res = await fetch(url, { headers: { 'User-Agent': UA }, redirect: 'follow' })
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const length = res.headers.get('content-length')
      if (length !== null && Number(length) !== size) {
        throw new Error(`Content-Length ${length} 与本地 ${size} 不一致`)
      }
      try {
        await res.body?.cancel()
      } catch {
        // 已经读完/关掉的流，忽略
      }
      return
    } catch (err) {
      if (attempt === 3) throw new Error(`下载地址校验失败 ${url}: ${err.message}`)
      warn(`校验 ${url} 第 ${attempt} 次失败，3s 后重试: ${err.message}`)
      await sleep(3000)
    }
  }
}

/** 写 Gitee 仓库里的清单：有就 PUT（update，需要 sha），没有就 POST（create）。 */
async function writeGiteeManifest(branch, manifest, label) {
  const content = Buffer.from(JSON.stringify(manifest, null, 2) + '\n').toString('base64')
  const path = `/repos/${GITEE_REPO}/contents/${UPDATES_DIR}/${ASSET_MANIFEST}`
  const message = `release: ${label}`

  let sha = ''
  try {
    const current = await gitee(path, { query: { ref: branch } })
    sha = current?.sha || ''
  } catch (err) {
    // 空仓库/分支还不存在时也可能返回 400，都当「文件不存在」处理
    if (err.status !== 404 && err.status !== 400) throw err
  }

  if (sha) {
    await gitee(path, { method: 'PUT', form: { content, message, sha, branch } })
  } else {
    try {
      await gitee(path, { method: 'POST', form: { content, message, branch } })
    } catch (err) {
      // 仓库是空仓库（没有 README）时分支还不存在，去掉 branch 让 Gitee 在默认分支创建
      warn(`首次创建清单时带 branch 失败，改为在默认分支创建: ${err.message}`)
      await gitee(path, { method: 'POST', form: { content, message } })
    }
  }
  log(`已写入 Gitee 清单 ${UPDATES_DIR}/${ASSET_MANIFEST}（${label}）`)
}

/** 把清单提交回 GitHub main：老客户端（只认 GitHub 端点）也能从 Gitee 下包。 */
function writeGithubManifest(manifest, label) {
  if (NO_GITHUB) {
    log('[跳过] 不提交 GitHub 清单（--no-github）')
    return
  }
  const path = join(ROOT, UPDATES_DIR, ASSET_MANIFEST)
  const next = JSON.stringify(manifest, null, 2) + '\n'
  const previous = existsSync(path) ? readFileSync(path, 'utf8') : ''
  if (previous === next) {
    log('GitHub 清单内容没有变化，跳过提交')
    return
  }

  fetchBranch(GITHUB_BRANCH)
  git(['checkout', '-B', GITHUB_BRANCH, `origin/${GITHUB_BRANCH}`])
  writeFileSync(path, next)
  git(['config', 'user.name', 'github-actions[bot]'])
  git(['config', 'user.email', '41898282+github-actions[bot]@users.noreply.github.com'])
  git(['add', `${UPDATES_DIR}/${ASSET_MANIFEST}`])
  git(['commit', '-m', `release: ${label} [skip ci]`])
  git(['push', 'origin', `HEAD:${GITHUB_BRANCH}`])
  log(`已提交 GitHub 清单 ${UPDATES_DIR}/${ASSET_MANIFEST}（${label}）`)
}

/** 拿到（必要时创建）这次的 Gitee 发行版；Gitee 的发行版必须挂在已有 tag 上。 */
async function ensureRelease(branch, tag, manifest) {
  try {
    // 注意：Gitee 对「tag 不存在」可能返回 200 + 空 body（而不是 404），所以要判空
    const release = await gitee(`/repos/${GITEE_REPO}/releases/tags/${encodeURIComponent(tag)}`)
    if (release && release.id) {
      log(`复用 Gitee 发行版: ${release.tag_name} (id=${release.id})`)
      return release
    }
  } catch (err) {
    if (err.status !== 404) throw err
  }

  // 先建 tag；重复运行时 tag 已存在，报错忽略即可
  try {
    await gitee(`/repos/${GITEE_REPO}/tags`, {
      method: 'POST',
      form: { refs: branch, tag_name: tag, tag_message: '' },
    })
    log(`已创建 Gitee tag: ${tag}`)
  } catch (err) {
    warn(`创建 Gitee tag 失败（可能已存在，继续）: ${err.message}`)
  }

  const created = await withRetry(
    () =>
      gitee(`/repos/${GITEE_REPO}/releases`, {
        method: 'POST',
        form: {
          tag_name: tag,
          name: tag,
          body: manifest.notes || `ws-workbench ${tag}`,
          target_commitish: branch,
          prerelease: false,
        },
      }),
    '创建 Gitee 发行版'
  )
  let release = created && created.id ? created : null
  if (!release) {
    // 创建接口偶尔不回 body，按 tag 再读一次
    release = await gitee(`/repos/${GITEE_REPO}/releases/tags/${encodeURIComponent(tag)}`)
  }
  if (!release || !release.id) throw new Error(`创建 Gitee 发行版后读不到发行版（tag ${tag}）`)
  log(`已创建 Gitee 发行版: ${release.tag_name} (id=${release.id})`)
  return release
}

/** 把清单里已镜像成功的平台换成 Gitee 地址，其余保持原样（GitHub 兜底）。 */
function buildMirroredManifest(base, baseUrl, uploaded) {
  const platforms = {}
  for (const [key, entry] of Object.entries(base.platforms || {})) {
    const asset = uploaded[key]
    platforms[key] = asset ? { ...entry, url: `${baseUrl}/${asset}` } : entry
  }
  return { ...base, platforms }
}

async function main() {
  if (!DRY_RUN && !GITEE_TOKEN) fail('缺少 GITEE_TOKEN（Gitee 私人令牌）；CI 里应配置 secrets.GITEE_TOKEN')

  const base = readBaseManifest()
  // 发版时版本号来自 tag（--version）；补镜像时不传，直接用清单里的版本
  const version = (VERSION || base.version || '').trim()
  if (!version) fail('缺少版本号：--version X.Y.Z，且清单里也没有 version 字段')
  if (!/^\d+\.\d+\.\d+$/.test(version)) fail(`版本号必须是 X.Y.Z 形式: ${version}`)
  if (base.version !== version) {
    fail(
      `清单版本(${base.version})和指定版本(${version})不一致；` +
        '三个构建 job 可能没把 updates/latest.json 合并推上去'
    )
  }
  if (Object.keys(base.platforms || {}).length === 0) fail('清单里没有任何平台条目')

  const { found: assets, installers } = locateAssets(version)

  const tag = `v${version}`
  log(`版本: ${tag}   产物目录: ${DIR}`)
  log(`Gitee 仓库: ${GITEE_REPO}`)

  if (DRY_RUN) {
    log('[dry-run] 只打印计划，不写任何远端')
    for (const platform of PLATFORMS) {
      const local = assets[platform.key]
      log(
        `[dry-run] ${platform.key}: ` +
          (local ? `上传 ${basename(local)} → releases/download/${tag}/${platform.asset}` : '缺少产物，保持 GitHub 地址')
      )
    }
    for (const installer of installers) {
      log(`[dry-run] ${installer.label}: 上传 ${installer.name} → releases/download/${tag}/${installer.name}`)
    }
    if (installers.length === 0) log('[dry-run] 没有找到手工安装包（dmg / msi），只镜像更新载荷')
    return
  }

  let branch = await resolveBranch()

  // 1) 先写一份「下载地址还是 GitHub」的清单：保证 Gitee 上的版本号永远是最新的
  //    （空仓库没初始化 README 时，这一步会顺手把默认分支建出来）
  await withRetry(() => writeGiteeManifest(branch, base, `${tag}（GitHub 地址）`), '写入 Gitee 清单（预写）')

  // 空仓库刚建出默认分支，重新解析一次，后面所有地址都按真实分支拼
  branch = await resolveBranch()
  const baseUrl = `${downloadBase()}/${tag}`
  const target = manifestUrl(branch)
  log(`Gitee 清单地址: ${target}`)

  // 2) 建 tag + 发行版
  const release = await ensureRelease(branch, tag, base)

  // 3) 上传 + 校验；失败的平台保持 GitHub 地址
  const uploaded = {}
  const failures = []
  for (const platform of PLATFORMS) {
    const local = assets[platform.key]
    if (!local) continue
    const size = statSync(local).size
    try {
      await uploadAttachment(release, local, platform.asset)
      await verifyRemote(`${baseUrl}/${platform.asset}`, size)
      uploaded[platform.key] = platform.asset
      log(`\x1b[32m${platform.key} 已镜像\x1b[0m → ${baseUrl}/${platform.asset}`)
    } catch (err) {
      failures.push(`${platform.key}: ${err.message}`)
      warn(`${platform.key} 镜像失败，清单里保留 GitHub 地址: ${err.message}`)
    }
  }

  // 3b) 上传手工安装包（dmg / msi）：只是发行版附件，不进更新清单，
  //     失败不影响客户端更新，但会在下面的失败汇总里标出来。
  //     三个包并行传：Gitee 对单连接限速很狠（实测 20-30KB/s），串行要半小时。
  await Promise.all(
    installers.map(async (installer) => {
      const size = statSync(installer.path).size
      try {
        await uploadAttachment(release, installer.path, installer.name)
        await verifyRemote(`${baseUrl}/${installer.name}`, size)
        log(`\x1b[32m${installer.label} 已镜像\x1b[0m → ${baseUrl}/${installer.name}`)
      } catch (err) {
        failures.push(`${installer.name}: ${err.message}`)
        warn(`${installer.label} 镜像失败（不影响更新清单）: ${err.message}`)
      }
    })
  )

  // 4) 把成功平台的下载地址换成 Gitee，写回 Gitee 清单和 GitHub 清单
  const mirrored = buildMirroredManifest(base, baseUrl, uploaded)
  await withRetry(() => writeGiteeManifest(branch, mirrored, tag), '写入 Gitee 清单')
  writeGithubManifest(mirrored, `${tag} 清单改用 Gitee 镜像`)

  // 5) 确认客户端配的端点真的能读到清单（分支写错时最容易在这里发现）
  try {
    const endpoints = JSON.parse(readFileSync(join(ROOT, 'src-tauri/tauri.conf.json'), 'utf8'))?.plugins
      ?.updater?.endpoints
    if (Array.isArray(endpoints) && endpoints.length > 0 && !endpoints.includes(target)) {
      warn(`tauri.conf.json 的更新端点里没有 ${target}；客户端不会读到这份 Gitee 清单`)
    } else {
      const res = await fetch(target, { headers: { 'User-Agent': UA } })
      if (res.ok) log(`客户端清单端点可读: ${target}`)
      else warn(`客户端清单端点返回 HTTP ${res.status}: ${target}`)
    }
  } catch (err) {
    warn(`校验客户端清单端点失败（不影响发布）: ${err.message}`)
  }

  if (failures.length > 0) {
    console.error(
      `\x1b[31m[gitee-sync] 以下附件没有镜像成功（更新载荷失败的平台在清单里保留 GitHub 地址）：\n  ${failures.join('\n  ')}\x1b[0m`
    )
    process.exitCode = 1
    return
  }
  const extraNames = installers.map((installer) => installer.name)
  log(
    `\x1b[32m镜像完成\x1b[0m ${tag}：${Object.keys(uploaded).join(', ')}` +
      (extraNames.length > 0 ? ` + ${extraNames.join(', ')}` : '')
  )
}

main().catch((err) => fail(err.message))
