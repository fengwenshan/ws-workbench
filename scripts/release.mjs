#!/usr/bin/env node
/**
 * 发布脚本：构建（含更新签名）→ 上传安装包到 GitHub 发行版 → 生成/合并 latest.json 并提交
 *
 * 用法：
 *   node scripts/release.mjs                 # 构建当前平台 + 发布
 *   node scripts/release.mjs --no-publish    # 只构建，产物落在 release/
 *   node scripts/release.mjs --no-publish --no-sign   # 没有签名私钥时：只出一个能装的包，
 *                                            # 不生成 .sig、不写 updates/latest.json（仅本机试装）
 *   node scripts/release.mjs --dmg           # macOS 额外生成 dmg（Intel / Apple Silicon 各一份）
 *   node scripts/release.mjs --target x86_64-apple-darwin     # 在 Apple Silicon 上交叉编 Intel 版
 *   node scripts/release.mjs --target aarch64-apple-darwin    # Apple Silicon 版
 *
 * Windows 的安装包（NSIS 的 exe + MSI）必须在 Windows 主机上构建：Tauri 打 MSI
 * 依赖 WiX，跨平台交叉编译拿不到；脚本会自动把 exe 作为更新载荷、MSI 作为
 * 手工安装包一起上传。
 *
 * 环境变量：
 *   TAURI_BUNDLES=app  只打 app、跳过 Tauri 的 dmg 打包脚本
 *                      （受限环境无法挂载 /Volumes 或写裸盘时必需，配合 --dmg 使用）
 *   TAURI_SIGNING_PRIVATE_KEY  签名私钥的内容；不设时回退读 ~/.tauri/ws-workbench.key
 *   GITHUB_TOKEN               GitHub 令牌（CI 里用 Actions 自动注入的那个），
 *                              创建发行版、上传附件、推送更新清单都需要
 *
 * 两个平台分别在各自可用的环境里执行即可：脚本会把本平台条目「合并」进
 * updates/latest.json，不会覆盖另一个平台。
 *
 * GitHub 侧：安装包作为发行版附件上传，更新清单提交到 updates/，
 * 客户端从 raw.githubusercontent.com 上的固定文件读取清单，再按清单里的
 * github.com/<repo>/releases/download 地址下载安装包。
 *
 * Gitee 镜像不在这里做：三个平台的构建 job 并行跑，镜像要等安装包都齐了
 * 才能开始，所以由 release.yml 里串行的 gitee-sync job 调 scripts/gitee-sync.mjs：
 * 把安装包传到 Gitee 同名 tag 的发行版，再把两份清单的下载地址改写成 Gitee。
 */
import { execFileSync } from 'node:child_process'
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  copyFileSync,
  chmodSync,
  writeFileSync,
  rmSync,
  statSync,
  symlinkSync,
} from 'node:fs'
import { join, dirname, resolve } from 'node:path'
import { tmpdir, homedir } from 'node:os'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'
import { request as httpsRequest } from 'node:https'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const TAURI_DIR = join(ROOT, 'src-tauri')
const OUT_DIR = join(ROOT, 'release')
/** 更新产物在仓库里的目录（会被提交，客户端通过 raw 地址读取） */
const UPDATES_DIR = 'updates'

const REPO = process.env.REPO || 'fengwenshan/ws-workbench'
const BRANCH = process.env.GITHUB_BRANCH || 'main'
const SKIP_PUBLISH = process.argv.includes('--no-publish')
/**
 * 跳过更新签名：只产出一个能装的安装包（本机试装用），不生成 `.sig`、不写
 * `updates/latest.json`。没有签名私钥时用这条路径，别把它放进更新通道。
 */
const NO_SIGN = process.argv.includes('--no-sign')
/** 更新清单的公开地址（公开仓库，无需凭据） */
const RAW_BASE = `https://raw.githubusercontent.com/${REPO}/${BRANCH}/${UPDATES_DIR}`
/** 发行版附件的下载地址前缀；URL 带 tag，长期稳定 */
const RELEASE_DOWNLOAD = `https://github.com/${REPO}/releases/download`

/** GitHub 令牌：创建发行版、上传附件、推送更新清单必需（CI 里由 Actions 注入） */
const GITHUB_TOKEN = process.env.GITHUB_TOKEN || ''
const GITHUB_API = 'https://api.github.com'
/** GitHub 的资产上传走独立域名 */
const GITHUB_UPLOAD = 'https://uploads.github.com'

const require = createRequire(import.meta.url)
/**
 * Tauri CLI 的 JS 入口：用 `node <entry> build ...` 调用。
 * 不走 `pnpm exec tauri`——Windows 上 pnpm 是 .cmd 包一层，execFileSync 直接
 * 起不来；本机 / CI（macOS、Windows）都装了 Node，这条路两边通用。
 */
const TAURI_CLI = require.resolve('@tauri-apps/cli/tauri.js')

// 私钥位置：默认 `~/.tauri/ws-workbench.key`，可用 TAURI_SIGNING_PRIVATE_KEY_PATH 覆盖。
// 必须用 os.homedir()：Windows 上没有 HOME 环境变量，原来写 `join(process.env.HOME || '', …)`
// 会退化成相对路径 `.tauri/ws-workbench.key`，于是「按提示生成到 ~/.tauri」也永远找不到。
const KEY_PATH = process.env.TAURI_SIGNING_PRIVATE_KEY_PATH
  ? resolve(process.env.TAURI_SIGNING_PRIVATE_KEY_PATH)
  : join(homedir(), '.tauri', 'ws-workbench.key')

/** 更新清单的固定文件名 */
const ASSET_MANIFEST = 'latest.json'

function argValue(flag) {
  const i = process.argv.indexOf(flag)
  return i >= 0 ? process.argv[i + 1] : ''
}

const TARGET = process.env.TAURI_TARGET || argValue('--target') || ''
/** 额外生成 macOS 的 dmg 安装包（见 createDmg 的说明） */
const WANT_DMG = process.argv.includes('--dmg')
/**
 * 版本号覆盖：CI 由 tag 触发时传入 tag 对应的版本号，避免修改文件造成的提交回环。
 * 通过 tauri build --config 注入，构建出的 app 与清单版本保持一致。
 */
const VERSION_OVERRIDE = argValue('--version') || process.env.RELEASE_VERSION || ''

function log(msg) {
  console.log(`\x1b[36m[release]\x1b[0m ${msg}`)
}
function fail(msg) {
  console.error(`\x1b[31m[release] 失败: ${msg}\x1b[0m`)
  process.exit(1)
}

/** 从 minisign 公钥文本里取 key id（`untrusted comment: minisign public key: XXXX`） */
function minisignKeyId(text) {
  const m = text.match(/minisign public key:\s*([0-9A-Fa-f]+)/)
  return m ? m[1].toUpperCase() : ''
}

/** tauri.conf.json 里 updater.pubkey 的 key id；读不到就返回空串（不阻断构建） */
function configuredUpdaterKeyId() {
  try {
    const conf = JSON.parse(readFileSync(join(TAURI_DIR, 'tauri.conf.json'), 'utf8'))
    const encoded = conf?.plugins?.updater?.pubkey || ''
    if (!encoded) return ''
    return minisignKeyId(Buffer.from(encoded, 'base64').toString('utf8'))
  } catch {
    return ''
  }
}

/** 当前构建目标的平台信息；product 用于精确匹配产物，避免选中改名前的旧文件 */
function resolvePlatform(product) {
  const isWindowsTarget = TARGET.includes('windows') || process.platform === 'win32'
  const archRaw = TARGET
    ? TARGET.split('-')[0] // x86_64 / aarch64 / i686
    : process.arch

  // Tauri 更新清单的键用 Rust 目标三元组里的架构名（windows-x86_64、darwin-aarch64），
  // 而 Node 的 process.arch 是 x64 / arm64 / ia32，先统一映射一次，
  // 否则原生构建会写出 windows-x64 这种客户端认不出来的键。
  const RUST_ARCH = {
    x64: 'x86_64',
    arm64: 'aarch64',
    ia32: 'i686',
    x86_64: 'x86_64',
    aarch64: 'aarch64',
    i686: 'i686',
  }
  const arch = RUST_ARCH[archRaw] || archRaw

  if (isWindowsTarget) {
    return {
      key: `windows-${arch}`,
      bundleSubdir: 'nsis',
      // 优先 NSIS 的 -setup.exe，其次 MSI
      matchBundle: (f) =>
        f.startsWith(product) && (f.endsWith('-setup.exe') || f.endsWith('.msi')),
      matchSig: (f) =>
        f.startsWith(product) && (f.endsWith('-setup.exe.sig') || f.endsWith('.msi.sig')),
      assetFile: 'ws-workbench-setup.exe',
      label: 'Windows',
    }
  }
  if (process.platform === 'darwin' || TARGET.includes('apple')) {
    return {
      key: `darwin-${arch}`,
      bundleSubdir: 'macos',
      matchBundle: (f) => f.startsWith(product) && f.endsWith('.app.tar.gz'),
      matchSig: (f) => f.startsWith(product) && f.endsWith('.app.tar.gz.sig'),
      // 两个架构各发一份，文件名带架构；更新清单里按 darwin-<arch> 分键
      assetFile: `ws-workbench_${arch}.app.tar.gz`,
      // macOS 额外提供 dmg 作为人工安装包（更新载荷用的是 .app.tar.gz）
      installerSubdir: 'dmg',
      matchInstaller: (f) => f.startsWith(product) && f.endsWith('.dmg'),
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

function readConf() {
  return JSON.parse(readFileSync(join(TAURI_DIR, 'tauri.conf.json'), 'utf8'))
}

function readVersion() {
  if (VERSION_OVERRIDE) return VERSION_OVERRIDE
  const conf = readConf()
  if (!conf.version) fail('tauri.conf.json 里没有 version 字段')
  return conf.version
}

/**
 * 用 hdiutil makehybrid 生成 dmg。
 *
 * Tauri 自带的 bundle_dmg.sh 需要挂载临时卷到 /Volumes 并写裸盘设备，
 * 在受限环境（如沙箱）里会失败。makehybrid 直接从目录生成镜像，
 * 不挂载、不写裸盘，再用 convert 压缩，可绕过该限制。
 * 产物缺少「拖入应用程序」的美化背景，但功能完全正常。
 */
function createDmg(platform, version) {
  if (!platform.key.startsWith('darwin-')) return null

  const product = readConf().productName || 'app'
  const dir = bundleDir(platform)
  const appName = `${product}.app`
  if (!existsSync(join(dir, appName))) {
    log(`未找到 ${appName}，跳过 dmg 生成`)
    return null
  }

  const arch = platform.key.replace('darwin-', '')
  // 产物目录跟着 target 走：交叉编 Intel 版时不能写进 host 的 target/release
  const outDir = join(dirname(dir), 'dmg')
  mkdirSync(outDir, { recursive: true })
  const outBase = join(outDir, `${product}_${version}_${arch}`)

  // 暂存目录：.app + 指向 /Applications 的快捷方式
  const stage = mkdtempSync(join(tmpdir(), 'dmg-stage-'))
  const raw = join(tmpdir(), `dmg-raw-${Date.now()}.dmg`)
  try {
    execFileSync('cp', ['-R', join(dir, appName), join(stage, appName)], { stdio: 'inherit' })
    symlinkSync('/Applications', join(stage, 'Applications'))

    // 安装包自带的辅助文件：都放在 dmg 根目录，不随 app 一起被删除。
    // - 安装说明.txt：卸载/安装两种方式 + 手动清除隔离属性的命令；
    // - 安装 ws-workbench.command：一键安装（复制 + 清除隔离属性 + 启动）；
    // - 卸载 ws-workbench.command：先退出进程，再按用户选择删应用/数据。
    const dmgExtras = [
      { source: '安装说明.txt', name: '安装说明.txt', mode: 0o644 },
      { source: 'install-ws-workbench.command', name: '安装 ws-workbench.command', mode: 0o755 },
      { source: 'uninstall-ws-workbench.command', name: '卸载 ws-workbench.command', mode: 0o755 },
    ]
    for (const extra of dmgExtras) {
      const sourcePath = join(ROOT, 'scripts/macos', extra.source)
      if (!existsSync(sourcePath)) {
        throw new Error(`缺少 dmg 辅助文件: ${sourcePath}`)
      }
      copyFileSync(sourcePath, join(stage, extra.name))
      chmodSync(join(stage, extra.name), extra.mode)
    }

    log('生成 dmg（makehybrid + UDZO 压缩）…')
    // convert 不会覆盖已存在的输出，重复构建时要先删掉
    rmSync(`${outBase}.dmg`, { force: true })
    execFileSync(
      'hdiutil',
      ['makehybrid', '-hfs', '-hfs-volume-name', product, '-o', raw, stage],
      { stdio: 'inherit' }
    )
    execFileSync('hdiutil', ['convert', raw, '-format', 'UDZO', '-o', outBase], {
      stdio: 'inherit',
    })

    const dmgPath = `${outBase}.dmg`
    if (!existsSync(dmgPath)) {
      log('dmg 未生成')
      return null
    }
    const mb = (statSync(dmgPath).size / 1048576).toFixed(1)
    log(`已生成 dmg: ${dmgPath} (${mb}MB)`)
    copyFileSync(dmgPath, join(OUT_DIR, `${product}_${version}_${arch}.dmg`))
    return dmgPath
  } finally {
    rmSync(stage, { recursive: true, force: true })
    rmSync(raw, { force: true })
  }
}

function build() {
  // 签名私钥：CI 里通过环境变量传入内容；本地则从 ~/.tauri 下的文件读取。
  // 注意打包器只识别 TAURI_SIGNING_PRIVATE_KEY（内容或路径），
  // TAURI_SIGNING_PRIVATE_KEY_PATH 不生效，实测会报 "no private key"。
  let privateKey = process.env.TAURI_SIGNING_PRIVATE_KEY || ''
  if (NO_SIGN) {
    log('--no-sign：跳过更新签名，产物只能本机试装（不生成 .sig，也不写 updates/latest.json）')
    if (privateKey) log('提示：环境变量里其实有签名私钥，去掉 --no-sign 就能正常签名')
  } else if (privateKey) {
    log('使用环境变量中的签名私钥')
  } else {
    if (!existsSync(KEY_PATH)) {
      fail(
        `找不到签名私钥: ${KEY_PATH}\n` +
          `生成一对新密钥: npx tauri signer generate -w "${KEY_PATH}"\n` +
          '（密钥设了密码时，再设环境变量 TAURI_SIGNING_PRIVATE_KEY_PASSWORD）\n' +
          '优先找回原来那把私钥：换新密钥就得同步改 src-tauri/tauri.conf.json 的 ' +
          'updater.pubkey，已装旧版本的机器会验签失败、再也收不到自动更新。'
      )
    }
    privateKey = readFileSync(KEY_PATH, 'utf8')
    log(`使用签名私钥: ${KEY_PATH}`)
    // 私钥和配置里的 pubkey 必须是同一对。换了新密钥却忘了改配置时，包能装、
    // 能发，但老客户端验签失败，症状要等用户点「检查更新」才暴露，这里提前拦住。
    const pubPath = `${KEY_PATH}.pub`
    if (existsSync(pubPath)) {
      const configuredId = configuredUpdaterKeyId()
      const keyId = minisignKeyId(readFileSync(pubPath, 'utf8'))
      if (configuredId && keyId && configuredId !== keyId) {
        fail(
          '签名密钥与 tauri.conf.json 的 updater.pubkey 不是同一对\n' +
            `  私钥: ${keyId}（${pubPath}）\n` +
            `  配置: ${configuredId}（src-tauri/tauri.conf.json）\n` +
            '用回原来那把私钥；确实要换密钥，就把 .pub 内容 Base64 后写进 updater.pubkey，' +
            '并接受「已装旧版本的机器只能手动重装」。'
        )
      }
    }
  }

  // 必须带 exec：package.json 里有个同名的 "tauri" 脚本（值为 "tauri dev"），
  // 直接 `pnpm tauri build` 会被解析成 `tauri dev build` —— 那是 dev 模式，
  // 起来后永不退出，CI 会一直挂在构建步骤上、出不了安装包。
  const args = ['build']
  if (TARGET) args.push('--target', TARGET)
  // 配置覆盖（合并到默认配置里，不改动源文件）：版本号 + 是否产出更新签名产物
  const overrides = {}
  if (VERSION_OVERRIDE) overrides.version = VERSION_OVERRIDE
  if (NO_SIGN) overrides.bundle = { createUpdaterArtifacts: false }
  if (Object.keys(overrides).length) args.push('--config', JSON.stringify(overrides))
  // 可用 TAURI_BUNDLES=app 跳过 dmg（例如受限环境无法创建 dmg 时）；
  // 更新通道只需要 .app.tar.gz，不依赖 dmg。
  if (process.env.TAURI_BUNDLES) args.push('--bundles', process.env.TAURI_BUNDLES)
  // 在非 Windows 主机上交叉编译 Windows：需要 cargo-xwin + nsis + llvm
  if (TARGET.includes('windows') && process.platform !== 'win32') {
    args.push('--runner', 'cargo-xwin')
  }

  log(`开始构建: tauri ${args.join(' ')}`)

  const env = privateKey
    ? { ...process.env, TAURI_SIGNING_PRIVATE_KEY: privateKey }
    : { ...process.env }

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

  execFileSync(process.execPath, [TAURI_CLI, ...args], { cwd: ROOT, stdio: 'inherit', env })
}

function findArtifacts(platform) {
  const dir = bundleDir(platform)
  if (!existsSync(dir)) {
    fail(`找不到构建产物目录: ${dir}\n请确认构建已成功完成`)
  }
  const files = readdirSync(dir)
  const bundle = files.find(platform.matchBundle)
  const sig = files.find(platform.matchSig)
  if (!bundle || (!sig && !NO_SIGN)) {
    fail(
      `在 ${dir} 里找不到安装包或签名文件\n` +
        `实际文件: ${files.join(', ')}\n` +
        '请确认 tauri.conf.json 的 bundle.createUpdaterArtifacts 为 true'
    )
  }
  return { bundlePath: join(dir, bundle), sigPath: sig ? join(dir, sig) : '' }
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

/** 读取某个 ref 上的 latest.json；文件或 ref 不存在时返回 null */
function readManifestAt(ref) {
  try {
    return JSON.parse(git(['show', `${ref}:${UPDATES_DIR}/${ASSET_MANIFEST}`]))
  } catch {
    return null
  }
}

function git(args, opts = {}) {
  return execFileSync('git', args, { cwd: ROOT, encoding: 'utf8', ...opts })
}

/** 生成更新说明：上一个 tag 到当前 HEAD 之间的提交 */
function buildChangelog(tag) {
  // 注意区间终点必须用 HEAD：本函数是在发行版创建「之前」调用的，
  // 此刻 tag 还不存在于本地，写成 ${prev}..${tag} 会因未知版本号而报错。
  let prev = ''
  try {
    const tags = git(['tag', '--sort=-creatordate']).trim().split('\n').filter(Boolean)
    prev = tags.find((t) => t !== tag) || ''
  } catch {
    // 没有 tag 时忽略
  }
  const range = prev ? `${prev}..HEAD` : 'HEAD'
  let lines = ''
  try {
    lines = git(['log', '--no-merges', '--pretty=format:* %s (%h)', range]).trim()
  } catch {
    lines = git(['log', '--no-merges', '-10', '--pretty=format:* %s (%h)']).trim()
  }
  const head = prev ? `自 ${prev} 以来的变更：` : '首次发布，包含以下提交：'
  return `${head}\n\n${lines || '（无提交记录）'}`
}

/** 本次要上传到发行版的附件 */
function collectAssets(platform) {
  const assets = [{ name: platform.assetFile, path: join(OUT_DIR, platform.assetFile) }]
  // Windows 额外上传 MSI（手工安装包；更新载荷用 NSIS 的 exe）
  if (platform.key.startsWith('windows-')) {
    const dir = bundleDir({ ...platform, bundleSubdir: 'msi' })
    if (existsSync(dir)) {
      const product = readConf().productName
      const f = readdirSync(dir).find(
        (name) => name.startsWith(product) && name.endsWith('.msi')
      )
      if (f) {
        // 顺手导出到 release/：CI 的 installers-* 产物只收 release/*，
        // 不导出的话 gitee-sync 拿不到 msi（2026-11-11 v1.0.22 就是这么漏的）。
        copyFileSync(join(dir, f), join(OUT_DIR, f))
        assets.push({ name: f, path: join(dir, f) })
      }
    }
  }
  // macOS 额外上传 dmg（人工安装包；更新载荷用的是 .app.tar.gz）
  if (platform.installerSubdir) {
    const dir = bundleDir({ ...platform, bundleSubdir: platform.installerSubdir })
    if (existsSync(dir)) {
      const f = readdirSync(dir).find(platform.matchInstaller)
      if (f) assets.push({ name: f, path: join(dir, f) })
    }
  }
  return assets
}

/**
 * 发一个 POST 请求，body 已经构造好。
 *
 * 这里刻意不用 fetch：上传大附件时 undici 的 headersTimeout 默认只有 300s，
 * 慢速上行会被误判成超时（UND_ERR_HEADERS_TIMEOUT）。
 * https.request 默认不设超时，这里只加一个 30 分钟的兜底空闲超时。
 */
function postBody(url, body, extraHeaders = {}) {
  return new Promise((resolve, reject) => {
    const req = httpsRequest(
      {
        method: 'POST',
        hostname: url.hostname,
        path: url.pathname + url.search,
        headers: { 'Content-Length': body.length, ...extraHeaders },
      },
      (res) => {
        let data = ''
        res.setEncoding('utf8')
        res.on('data', (c) => (data += c))
        res.on('end', () => {
          if (res.statusCode >= 200 && res.statusCode < 300) resolve()
          else reject(new Error(`HTTP ${res.statusCode}: ${data.slice(0, 300)}`))
        })
      }
    )
    req.setTimeout(1800000, () => {
      req.destroy(new Error('30 分钟无响应'))
    })
    req.on('error', reject)
    req.end(body)
  })
}

/** 上传带上重试：上行偶发中断时，清掉半成品再重来 */
async function uploadWithRetry(asset, doUpload, doRemove, attempts = 3) {
  for (let i = 1; i <= attempts; i++) {
    try {
      await doUpload(asset)
      log(`已上传 ${asset.name}`)
      return
    } catch (err) {
      if (i === attempts) throw err
      log(`第 ${i} 次上传 ${asset.name} 失败：${err.message}`)
      log(`等待后重试（${i}/${attempts - 1}）…`)
      await doRemove(asset.name)
      await new Promise((r) => setTimeout(r, 15000 * i))
    }
  }
}

/** 逐个上传资产：先删同名旧件，再带重试上传 */
async function pushAssets(assets, doUpload, doRemove) {
  for (const a of assets) {
    await doRemove(a.name)
    await uploadWithRetry(a, doUpload, doRemove)
  }
}

// ===== GitHub 发行版 =====

/** 调用 GitHub API（Bearer 认证走请求头） */
async function githubApi(path, options = {}) {
  const res = await fetch(`${GITHUB_API}${path}`, {
    ...options,
    headers: {
      Accept: 'application/vnd.github+json',
      Authorization: `Bearer ${GITHUB_TOKEN}`,
      'X-GitHub-Api-Version': '2022-11-28',
      ...(options.headers || {}),
    },
  })
  const text = await res.text()
  if (!res.ok) {
    const err = new Error(
      `GitHub API ${res.status} ${path.split('?')[0]}: ${text.slice(0, 300)}`
    )
    err.status = res.status
    throw err
  }
  return text ? JSON.parse(text) : null
}

/** 找到 tag 对应的发行版；不存在时返回 null（GitHub 对不存在的 tag 返回 404） */
async function findGithubRelease(tag) {
  try {
    return await githubApi(`/repos/${REPO}/releases/tags/${encodeURIComponent(tag)}`)
  } catch (err) {
    if (err.status === 404) return null
    throw err
  }
}

async function ensureGithubRelease(tag) {
  const existing = await findGithubRelease(tag)
  if (existing) {
    log(`GitHub 发行版 ${tag} 已存在，更新资产 …`)
    return existing
  }
  log(`创建 GitHub 发行版 ${tag} …`)
  try {
    return await githubApi(`/repos/${REPO}/releases`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        tag_name: tag,
        name: tag,
        body: buildChangelog(tag),
        target_commitish: BRANCH,
        prerelease: false,
      }),
    })
  } catch (err) {
    // 两个平台并行时可能同时判定「不存在」而都想创建（返回 422），复用对方建好的
    const release = await findGithubRelease(tag)
    if (!release) throw err
    log(`GitHub 发行版 ${tag} 已由另一个平台创建，复用`)
    return release
  }
}

async function removeGithubAsset(releaseId, name) {
  try {
    const assets = (await githubApi(`/repos/${REPO}/releases/${releaseId}/assets`)) || []
    for (const a of assets) {
      if (a.id && a.name === name) {
        await githubApi(`/repos/${REPO}/releases/assets/${a.id}`, { method: 'DELETE' })
        log(`已移除旧资产 ${a.name}`)
      }
    }
  } catch (err) {
    log(`清理资产 ${name} 失败（忽略）：${err.message}`)
  }
}

function uploadGithubAsset(releaseId, name, filePath) {
  const url = new URL(`${GITHUB_UPLOAD}/repos/${REPO}/releases/${releaseId}/assets`)
  url.searchParams.set('name', name)
  // GitHub 的资产接口要的是裸字节：文件名走 ?name=，body 就是文件本身。
  // 传 multipart 不会报错，但会把整个 body（连边界和报头）原样存成资产，
  // 下载下来是个坏文件——实测 size 会多出约 200 字节，content_type 显示 boundary。
  return postBody(url, readFileSync(filePath), {
    Authorization: `Bearer ${GITHUB_TOKEN}`,
    'Content-Type': 'application/octet-stream',
  })
}

/**
 * 把安装包上传到 GitHub 发行版。
 *
 * 这是唯一的发布目标：更新清单里的下载地址也指向这里。
 */
async function publishToGithub(tag, assets) {
  if (!GITHUB_TOKEN) {
    fail('发布需要 GITHUB_TOKEN 环境变量（GitHub 令牌，CI 里由 Actions 自动注入）')
  }
  const release = await ensureGithubRelease(tag)
  await pushAssets(
    assets,
    (a) => uploadGithubAsset(release.id, a.name, a.path),
    (n) => removeGithubAsset(release.id, n)
  )
  log(`GitHub 发行版已更新: https://github.com/${REPO}/releases/tag/${tag}`)
}

/**
 * 以远端（origin/<branch>）的清单为基线，合入本平台条目并写成文件。
 * 返回合并后的清单以及是否真的有改动。
 */
function stageManifest(version, platform, entry) {
  const base = readManifestAt(`origin/${BRANCH}`) || { platforms: {} }
  const manifest = {
    version,
    notes: `ws-workbench v${version}`,
    pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
    platforms: { ...(base.platforms || {}), [platform.key]: entry },
  }
  mkdirSync(join(ROOT, UPDATES_DIR), { recursive: true })
  writeFileSync(join(ROOT, UPDATES_DIR, ASSET_MANIFEST), JSON.stringify(manifest, null, 2) + '\n')
  git(['add', UPDATES_DIR])
  const changed = Boolean(git(['diff', '--cached', '--name-only']).trim())
  return { manifest, changed }
}

/**
 * 提交更新清单。
 *
 * macOS 与 Windows 是两个并行 job，会同时改同一个文件。所以每次提交前都先把
 * HEAD 快进到远端最新、以远端版本为基线合并本平台条目；推送被拒就把这次提交
 * 撤销后重来。这样两个 job 不论谁先完成都不会互相顶掉，也不会留下多余提交。
 */
async function commitManifest(version, platform, entry) {
  const rel = `${UPDATES_DIR}/${ASSET_MANIFEST}`
  const msg = `release: v${version} (${platform.key}) [skip ci]`

  for (let attempt = 1; attempt <= 6; attempt++) {
    // 只回滚清单文件本身，不碰工作区里其他改动（本地手动发布时才安全）
    try {
      git(['checkout', 'HEAD', '--', rel])
    } catch {
      // 清单还没被跟踪过，忽略
    }
    git(['fetch', 'origin', BRANCH])
    try {
      git(['merge', '--ff-only', `origin/${BRANCH}`])
    } catch {
      // 本地已有分叉（手动发布时可能出现），下面以远端为基线合并即可
    }

    const { manifest, changed } = stageManifest(version, platform, entry)
    if (changed) {
      git(['commit', '-m', msg, '--', UPDATES_DIR])
      log('推送更新清单 …')
    }

    try {
      git(['push', 'origin', `HEAD:${BRANCH}`], { stdio: 'inherit' })
      log(`清单已包含平台: ${Object.keys(manifest.platforms).join(', ')}`)
      return manifest
    } catch (err) {
      // 撤销本次提交（改动留在暂存区，下一轮会连同新基线一起重写）
      if (changed) git(['reset', '--soft', 'HEAD~1'])
      else log(`推送清单失败：${String(err.message).split('\n')[0]}`)
      if (attempt === 6) {
        fail(
          `更新清单推送连续 ${attempt} 次失败，放弃\n` +
            '若在本机手动发布：先 git pull 同步远端，并确认没有未提交的改动。'
        )
      }
      log(`第 ${attempt} 次推送被拒（另一个平台可能刚推过），${5 * attempt}s 后重试 …`)
      await new Promise((r) => setTimeout(r, 5000 * attempt))
    }
  }
}

/**
 * 发布：产物上传到 GitHub 发行版附件，更新清单提交到 updates/ 目录。
 * 清单必须留在固定地址上（raw.githubusercontent.com 的稳定文件），
 * 而它里面的下载地址指向带 tag 的发行版附件，长期稳定。
 */
async function publish(version, platform, entry) {
  const tag = `v${version}`
  const assets = collectAssets(platform)

  // 主发布目标：GitHub 发行版。客户端的更新下载走这里，必须成功。
  await publishToGithub(tag, assets)

  // 更新清单：两个平台 job 会同时改这个文件，交给 commitManifest 做同步 + 重试。
  const manifest = await commitManifest(version, platform, entry)

  log(`\x1b[32m发布完成\x1b[0m ${tag} (${platform.key})`)
  log('本次包含平台: ' + Object.keys(manifest.platforms).join(', '))
  log(`发行版页面: https://github.com/${REPO}/releases/tag/${tag}`)
  log(`客户端更新清单: ${RAW_BASE}/${ASSET_MANIFEST}`)
}

async function main() {
  const version = readVersion()
  const product = readConf().productName || 'app'
  const platform = resolvePlatform(product)

  log(`版本: ${version}`)
  log(`平台: ${platform.key} (${platform.label})`)
  log(`仓库: ${REPO}  分支: ${BRANCH}`)
  if (TARGET) log(`构建目标: ${TARGET}`)

  build()

  const { bundlePath, sigPath } = findArtifacts(platform)

  if (NO_SIGN) {
    // 未签名产物不能进更新通道：客户端拿到没有 signature 的条目会直接拒绝更新。
    if (!SKIP_PUBLISH) {
      fail('--no-sign 只能配合 --no-publish 使用：更新通道只接受签名产物')
    }
    mkdirSync(OUT_DIR, { recursive: true })
    copyFileSync(bundlePath, join(OUT_DIR, platform.assetFile))
    log(`已导出 release/${platform.assetFile}（未签名，只能本机试装）`)
    return
  }

  const signature = readFileSync(sigPath, 'utf8').trim()
  if (!signature) fail('签名文件为空，构建可能没有正确签名')

  mkdirSync(OUT_DIR, { recursive: true })
  copyFileSync(bundlePath, join(OUT_DIR, platform.assetFile))
  log(`已导出 release/${platform.assetFile}`)

  if (WANT_DMG) createDmg(platform, version)

  // 本平台在更新清单里的条目
  const entry = {
    signature,
    url: `${RELEASE_DOWNLOAD}/v${version}/${platform.assetFile}`,
  }

  // 供本地查看 / CI 存档的合并视图；真正提交的内容以 commitManifest 拉到的远端版本为准
  const published = readPublishedManifest()
  const platforms = { ...(published?.platforms || {}), [platform.key]: entry }
  const manifest = {
    version,
    notes: `ws-workbench v${version}`,
    pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
    platforms,
  }
  writeFileSync(join(OUT_DIR, ASSET_MANIFEST), JSON.stringify(manifest, null, 2) + '\n')
  log(`已生成 release/${ASSET_MANIFEST}（平台: ${Object.keys(platforms).join(', ')}）`)

  if (SKIP_PUBLISH) {
    log('已指定 --no-publish，跳过提交')
    return
  }
  await publish(version, platform, entry)
}

main().catch((err) => fail(err.message))
