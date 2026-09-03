#!/usr/bin/env node
// 校验 package-lock.json 中新增/变更包的 sha512 integrity 是否与 npm 注册表一致。
// 用于发布前检查：手写或转录损坏的 integrity 会在冷 `npm ci` 时才爆发，
// 这里通过 `npm view <pkg>@<version> dist.integrity` 对照，把失败提前到派发之前。
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')

function run(cmd, args) {
  return execFileSync(cmd, args, { cwd: root, encoding: 'utf8' }).trim()
}

// npm_execpath 指向 npm 的 CLI js，直接用 node 执行可跨平台避开 .cmd shim；
// 独立运行时退回裸名，Windows 上 .cmd 必须经 shell（Node 22+ 直接 spawn 会 EINVAL）。
const npmCli = String(process.env.npm_execpath || '').trim()
function runNpm(commandArgs) {
  if (npmCli) return run(process.execPath, [npmCli, ...commandArgs])
  const isWin = process.platform === 'win32'
  return execFileSync(isWin ? 'npm.cmd' : 'npm', commandArgs, {
    cwd: root,
    encoding: 'utf8',
    shell: isWin,
  }).trim()
}

// 默认基线：全部组件标签中最新的一个；lockfile 只在该标签之后才可能有变更。
function defaultBaseRef() {
  const tags = run('git', [
    'tag',
    '--list',
    'v*',
    'runtime-v*',
    'tui-v*',
    'app-v*',
    'npm-v*',
    '--sort=-creatordate',
  ])
    .split('\n')
    .filter(Boolean)
  return tags[0] || null
}

function parseLock(raw) {
  const lock = JSON.parse(raw)
  return new Map(Object.entries(lock.packages || {}))
}

const baseRef = process.argv[2] || defaultBaseRef()
const current = parseLock(readFileSync(resolve(root, 'package-lock.json'), 'utf8'))
const base = baseRef ? parseLock(run('git', ['show', `${baseRef}:package-lock.json`])) : new Map()

// 只校验 registry.npmjs.org 来源且 integrity 发生变化的条目，避免全量网络请求。
const changed = []
for (const [key, value] of current) {
  if (!key || !value?.integrity) continue
  if (!String(value.resolved || '').startsWith('https://registry.npmjs.org/')) continue
  const prev = base.get(key)
  if (
    prev &&
    prev.version === value.version &&
    prev.integrity === value.integrity &&
    prev.resolved === value.resolved
  )
    continue
  changed.push({
    // 嵌套依赖条目的包名是最后一个 node_modules 段
    name: key.split('node_modules/').pop(),
    version: value.version,
    integrity: value.integrity,
  })
}

if (!changed.length) {
  console.log(`lockfile integrity 校验通过（基线 ${baseRef || '无标签'}，无变更条目）。`)
  process.exit(0)
}

console.log(`对照注册表校验 ${changed.length} 个变更的 lockfile 条目（基线 ${baseRef}）…`)
const failures = []
for (const entry of changed) {
  let registryIntegrity = ''
  try {
    registryIntegrity = runNpm(['view', `${entry.name}@${entry.version}`, 'dist.integrity'])
  } catch (error) {
    failures.push(`${entry.name}@${entry.version}: 无法查询注册表（${error.message}）`)
    continue
  }
  if (registryIntegrity !== entry.integrity) {
    failures.push(
      `${entry.name}@${entry.version}: lockfile ${entry.integrity} 与注册表 ${registryIntegrity} 不一致`,
    )
  }
}

if (failures.length) {
  console.error('lockfile integrity 校验失败：')
  for (const failure of failures) console.error(`  - ${failure}`)
  process.exit(1)
}
console.log('lockfile integrity 校验通过。')
