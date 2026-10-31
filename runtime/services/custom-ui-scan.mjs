// 原生组件源码静态扫描（规格 5.7，嫁接自 tiles）：把源码里出现的网络主机与敏感用法和 manifest 的声明逐项对照，
// 供导入信任确认与工作室 describe 展示。
//
// 这是“看得见”的辅助，不是隔离：组件与应用同源执行，混淆过的代码可以绕过任何正则。界面文案如实说明，
// 结果里也不给“安全 / 不安全”的结论，只列出发现与是否声明。
// 只扫描文本资源（.js / .mjs / .css / .json / .svg）；单个文件只看前 512 KiB，单条摘录 ≤120 字符。
import { hostMatches } from './custom-ui-fetch-proxy.mjs'

const TEXT_FILE = /\.(?:m?js|css|json|svg)$/i
const MAX_SCAN_BYTES = 512 * 1024
const MAX_FINDINGS = 200
const MAX_HOSTS = 50

/**
 * 规则：severity 决定界面颜色。danger = 绕过 Pisper 接口直接联网或执行动态代码（标红）；
 * warn = 直接读写应用层状态（应用 API、Cookie、localStorage）；info = 动态导入等值得留意但常见的写法。
 * @type {{ rule: string, severity: 'danger' | 'warn' | 'info', pattern: RegExp, scripts: boolean }[]}
 */
const RULES = [
  // 直接调用全局 fetch（含 window./globalThis./self. 前缀）；ctx.net.fetch 前面是“.net.”，不算。
  {
    rule: 'fetch',
    severity: 'danger',
    pattern: /(?:(?<![\w$.])|\b(?:window|globalThis|self)\s*\.\s*)fetch\s*\(/g,
    scripts: true,
  },
  { rule: 'xhr', severity: 'danger', pattern: /\bXMLHttpRequest\b/g, scripts: true },
  {
    rule: 'websocket',
    severity: 'danger',
    pattern: /\b(?:WebSocket|EventSource)\b/g,
    scripts: true,
  },
  { rule: 'eval', severity: 'danger', pattern: /(?<![\w$.])eval\s*\(/g, scripts: true },
  { rule: 'function', severity: 'danger', pattern: /\bnew\s+Function\s*\(/g, scripts: true },
  { rule: 'cookie', severity: 'warn', pattern: /\bdocument\s*\.\s*cookie\b/g, scripts: true },
  {
    rule: 'local-storage',
    severity: 'warn',
    pattern: /\b(?:localStorage|sessionStorage|indexedDB)\b/g,
    scripts: true,
  },
  { rule: 'app-api', severity: 'warn', pattern: /['"`]\/api\//g, scripts: false },
  { rule: 'dynamic-import', severity: 'info', pattern: /\bimport\s*\(/g, scripts: true },
]

const URL_PATTERN =
  /\b(?:https?|wss?):\/\/([a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?)*)(?::\d{1,5})?/gi

/**
 * @typedef {{ rule: string, severity: 'danger' | 'warn' | 'info', file: string, line: number, excerpt: string }} ScanFinding
 * @typedef {{ host: string, declared: boolean, file: string, line: number }} ScanHost
 * @typedef {{
 *   status: 'ok',
 *   hosts: ScanHost[],
 *   findings: ScanFinding[],
 *   summary: { danger: number, warn: number, info: number, undeclaredHosts: number },
 *   truncated: boolean,
 *   skipped: string[],
 * }} ScanResult
 */

/** @param {string} text @param {number} index */
function lineOf(text, index) {
  let line = 1
  for (let cursor = text.indexOf('\n'); cursor !== -1 && cursor < index;) {
    line += 1
    cursor = text.indexOf('\n', cursor + 1)
  }
  return line
}

/** @param {string} text @param {number} index */
function excerptAt(text, index) {
  const start = text.lastIndexOf('\n', index) + 1
  const end = text.indexOf('\n', index)
  const line = text.slice(start, end === -1 ? undefined : end).trim()
  return line.length > 120 ? `${line.slice(0, 117)}…` : line
}

// 常见的文档 / 命名空间地址不是联网目标（SVG 的 xmlns、JSON Schema 的 $schema），不列为主机。
const NAMESPACE_HOSTS = new Set(['www.w3.org', 'json-schema.org'])

/**
 * @param {{ path: string, content: string | Uint8Array }[]} files
 * @param {{ capabilities?: { network?: { hosts?: string[] } | null } } | null} manifest 已规范化的 manifest；无效时传 null
 * @returns {ScanResult}
 */
export function scanComponentSource(files, manifest) {
  const declaredHosts = manifest?.capabilities?.network?.hosts || []
  /** @type {ScanFinding[]} */
  const findings = []
  /** @type {Map<string, ScanHost>} */
  const hosts = new Map()
  /** @type {string[]} */
  const skipped = []
  let truncated = false
  const sorted = [...files].sort((left, right) => (left.path < right.path ? -1 : 1))
  for (const file of sorted) {
    if (!TEXT_FILE.test(file.path)) continue
    const raw =
      typeof file.content === 'string' ? Buffer.from(file.content) : Buffer.from(file.content)
    if (raw.byteLength > MAX_SCAN_BYTES) {
      skipped.push(file.path)
      truncated = true
    }
    const text = raw.subarray(0, MAX_SCAN_BYTES).toString('utf8')
    const script = /\.m?js$/i.test(file.path)
    for (const { rule, severity, pattern, scripts } of RULES) {
      if (scripts && !script) continue
      pattern.lastIndex = 0
      for (const match of text.matchAll(pattern)) {
        if (findings.length >= MAX_FINDINGS) {
          truncated = true
          break
        }
        const index = match.index ?? 0
        findings.push({
          rule,
          severity,
          file: file.path,
          line: lineOf(text, index),
          excerpt: excerptAt(text, index),
        })
      }
    }
    URL_PATTERN.lastIndex = 0
    for (const match of text.matchAll(URL_PATTERN)) {
      const host = match[1].toLowerCase()
      if (NAMESPACE_HOSTS.has(host) || hosts.has(host)) continue
      if (hosts.size >= MAX_HOSTS) {
        truncated = true
        break
      }
      hosts.set(host, {
        host,
        // 与网络代理同一套匹配规则：扫描说“已声明”的主机，代理一定放行。
        declared: hostMatches(declaredHosts, host),
        file: file.path,
        line: lineOf(text, match.index ?? 0),
      })
    }
  }
  const count = (severity) => findings.filter((entry) => entry.severity === severity).length
  const hostList = [...hosts.values()]
  return {
    status: 'ok',
    hosts: hostList,
    findings,
    summary: {
      danger: count('danger'),
      warn: count('warn'),
      info: count('info'),
      undeclaredHosts: hostList.filter((entry) => !entry.declared).length,
    },
    truncated,
    skipped,
  }
}
