#!/usr/bin/env node
/**
 * 读取仓库根目录的 env / env.local，塞进 process.env，供发布脚本使用。
 *
 * 优先级：真实环境变量 > env.local > env
 *   - env 可以提交到仓库（里面只放空占位或公开默认值，例如 `GITEE_TOKEN=`）
 *   - env.local 被 .gitignore 的 `*.local` 忽略，用来放本机令牌（GITEE_TOKEN 等），
 *     这样 `pnpm release:beta` 就不用每次在命令行前指定令牌
 *
 * 只支持最简单的 `KEY=VALUE`（允许 `export KEY=VALUE`、单双引号包值、# 注释行），
 * 不做变量展开和多行值；写错的行直接忽略，不会阻断发布。
 */
import { existsSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')

// 后面的文件覆盖前面的同名变量；.local 总在最后，所以优先级最高
const FILES = ['.env', 'env', '.env.local', 'env.local']

const LINE = /^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/

function parse(content) {
  const values = {}
  for (const raw of content.split(/\r?\n/)) {
    const line = raw.trim()
    if (!line || line.startsWith('#')) continue
    const match = LINE.exec(line)
    if (!match) continue
    let value = match[2].trim()
    const quoted =
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    if (quoted) value = value.slice(1, -1)
    values[match[1]] = value
  }
  return values
}

const merged = {}
for (const name of FILES) {
  const file = join(ROOT, name)
  if (!existsSync(file)) continue
  Object.assign(merged, parse(readFileSync(file, 'utf8')))
}

for (const [key, value] of Object.entries(merged)) {
  // 命令行/CI 显式传的环境变量优先，env 文件只补空缺
  if (process.env[key] === undefined) process.env[key] = value
}
