import { createHash } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { parse } from '@babel/parser'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const args = process.argv.slice(2)
const referenceFlag = args.indexOf('--reference')
const reportFlag = args.indexOf('--report')
const reference = path.resolve(
  referenceFlag < 0
    ? path.join(root, '../pisper-release-parity-reference')
    : args[referenceFlag + 1],
)
const reportPath = path.resolve(
  reportFlag < 0 ? path.join(root, 'docs/reports/rust-release-parity.json') : args[reportFlag + 1],
)

async function filesBelow(directory, extension) {
  const entries = await readdir(directory, { withFileTypes: true })
  const groups = await Promise.all(
    entries.map((entry) => {
      const file = path.join(directory, entry.name)
      return entry.isDirectory()
        ? filesBelow(file, extension)
        : Promise.resolve(file.endsWith(extension) ? [file] : [])
    }),
  )
  return groups.flat().sort()
}

function walk(node, visitor) {
  if (!node || typeof node !== 'object') return
  if (typeof node.type === 'string') visitor(node)
  for (const [key, value] of Object.entries(node)) {
    if (['loc', 'tokens', 'comments'].includes(key)) continue
    if (Array.isArray(value)) value.forEach((item) => walk(item, visitor))
    else if (value && typeof value === 'object') walk(value, visitor)
  }
}

function normalizeRoute(value) {
  return value.replace(/:([A-Za-z][A-Za-z0-9_]*)/g, '{$1}')
}

function ownerFor(route) {
  if (/^\/api\/(?:workflows|workflow-runs|schedules)(?:\/|$)/.test(route))
    return 'workflows-schedules'
  if (
    /^\/api\/(?:memory|assets|game-assets|speech|workflow-media|sprite-engines|workflow-image)/.test(
      route,
    )
  )
    return 'memory-assets-media'
  if (/\/(?:approvals|execution-mode|permission|goal|plan|agents|team)(?:\/|$)/.test(route))
    return 'execution-goals-agents'
  return 'integration-runtime-platform'
}

// 路由声明只证明表面覆盖；行为、数据、工具、生命周期和平台验收必须另外提供证据。
const expected = []
const routeFiles = await filesBelow(path.join(reference, 'runtime/http/routes'), '.mjs')
for (const file of routeFiles) {
  const source = await readFile(file, 'utf8')
  const ast = parse(source, { sourceType: 'module' })
  walk(ast, (node) => {
    if (node.type !== 'ObjectExpression') return
    const properties = new Map(
      node.properties
        .filter((property) => property.type === 'ObjectProperty')
        .map((property) => [property.key.name ?? property.key.value, property.value]),
    )
    const method = properties.get('method')?.value
    const route = properties.get('path')?.value
    if (typeof method !== 'string' || typeof route !== 'string' || !route.startsWith('/api/'))
      return
    expected.push({
      method,
      path: route,
      normalizedPath: normalizeRoute(route),
      source: path.relative(reference, file).replaceAll(path.sep, '/'),
      line: node.loc.start.line,
      sourceContractSha256: createHash('sha256')
        .update(source.slice(node.start, node.end))
        .digest('hex'),
      owner: ownerFor(route),
    })
  })
}

const nativeDeclarations = []
for (const file of await filesBelow(path.join(root, 'runtime-rs/src'), '.rs')) {
  const source = await readFile(file, 'utf8')
  for (const match of source.matchAll(/\.route\(\s*"([^"]+)"/g)) {
    const start = match.index + match[0].length
    let end = start
    let depth = 1
    let quoted = false
    let escaped = false
    for (; end < source.length; end++) {
      const character = source[end]
      if (quoted) {
        if (escaped) escaped = false
        else if (character === '\\') escaped = true
        else if (character === '"') quoted = false
      } else if (character === '"') quoted = true
      else if (character === '(') depth++
      else if (character === ')' && --depth === 0) break
    }
    const declaration = source.slice(start, end)
    for (const method of declaration.matchAll(
      /\b(get|post|put|patch|delete|head|options|any)\s*\(/g,
    )) {
      nativeDeclarations.push({
        method: method[1].toUpperCase(),
        path: match[1],
        source: path.relative(root, file).replaceAll(path.sep, '/'),
        line: source.slice(0, match.index).split('\n').length,
      })
    }
  }
}

const byKey = new Map(expected.map((route) => [`${route.method} ${route.path}`, route]))
const requirements = [...byKey.values()].map((route) => ({
  ...route,
  declarations: nativeDeclarations.filter(
    (native) => native.path === route.normalizedPath && native.method === route.method,
  ),
  wiredRuntimeVerified: false,
  behaviorVerified: false,
  verificationStatus: 'pending-reference-contract-tests',
}))
const referenceCommit = execFileSync('git', ['-C', reference, 'rev-parse', 'HEAD'], {
  encoding: 'utf8',
}).trim()
const report = {
  objective:
    'Rust development branch must match release functionality, UI, contracts, storage, tools, lifecycle, and supported platforms.',
  status: 'incomplete',
  generatedAt: new Date().toISOString(),
  reference: { branch: 'release', commit: referenceCommit },
  scope: {
    http: 'Every release API route below, including methods, validation, payloads, errors, SSE and authorization.',
    frontend: 'Release components and routes restored; only engine identity adapter retained.',
    tools:
      'All release builtin and plugin tools, approvals, actual execution and failure behavior.',
    persistence:
      'Release formats, migrations, old data, restart, unknown fields and secret handling.',
    lifecycle:
      'Concurrency, cancellation, retries, queueing, scheduling, remote and channel resources.',
    platforms: 'Release-supported desktop, TUI, Web and mobile capability and packaging behavior.',
  },
  verificationBoundary:
    'Static declarations are inventory evidence only. No route or feature is marked equivalent without executable behavioral and integration evidence.',
  totals: {
    releaseRoutes: requirements.length,
    declarationPresent: requirements.filter((route) => route.declarations.length > 0).length,
    declarationMissing: requirements.filter((route) => route.declarations.length === 0).length,
    behaviorVerified: 0,
  },
  requirements,
}
await mkdir(path.dirname(reportPath), { recursive: true })
await writeFile(reportPath, JSON.stringify(report, null, 2) + '\n')
console.log(JSON.stringify({ status: report.status, referenceCommit, ...report.totals }, null, 2))
if (args.includes('--require-complete')) process.exitCode = 1
