// 原生浏览器验收的只读进程快照；PID 与启动时间一起用于判断回收，避免 PID 复用误报。
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { basename } from 'node:path'
const execute = promisify(execFile)

export function parsePosixProcesses(output) {
  return output.split('\n').flatMap((line) => {
    const match = line.match(
      /^\s*(\d+)\s+(\d+)\s+(\w+\s+\w+\s+\d+\s+\d{2}:\d{2}:\d{2}\s+\d{4})\s+(.+)$/,
    )
    if (!match) return []
    return [
      {
        pid: Number(match[1]),
        parent: Number(match[2]),
        createdUtc: match[3].replace(/\s+/g, ' '),
        path: match[4],
      },
    ]
  })
}
export async function posixProcesses() {
  const { stdout } = await execute('ps', ['-axo', 'pid=,ppid=,lstart=,comm='], {
    env: { ...process.env, LC_ALL: 'C', TZ: 'UTC' },
    maxBuffer: 8 * 1024 * 1024,
  })
  return parsePosixProcesses(stdout)
}
export function browserDescendants(rows, parent) {
  const found = new Set([parent])
  for (;;) {
    const size = found.size
    for (const row of rows) if (found.has(row.parent)) found.add(row.pid)
    if (size === found.size) break
  }
  return rows.filter(
    (row) =>
      row.pid !== parent &&
      found.has(row.pid) &&
      /chrome|chromium|msedge|brave|microsoft edge/i.test(basename(row.path)),
  )
}
const sameProcess = (one, two) => one.pid === two.pid && one.createdUtc === two.createdUtc

export function browserProcessRoots(rows, parent) {
  return browserDescendants(rows, parent).filter((row) => row.parent === parent)
}

export function browserProcessTree(rows, root) {
  const currentRoot = rows.find((row) => sameProcess(row, root))
  if (!currentRoot) return []
  return [currentRoot, ...browserDescendants(rows, root.pid)]
}

export function newBrowserProcessTree(before, current, parent) {
  const previous = browserProcessRoots(before, parent)
  const roots = browserProcessRoots(current, parent).filter(
    (row) => !previous.some((owned) => sameProcess(row, owned)),
  )
  // 驱动直接启动独立的浏览器根进程；渲染器后生子进程不能决定会话归属。
  if (roots.length !== 1)
    throw new Error(`Expected one newly owned browser root; found ${roots.length}.`)
  return { root: roots[0], rows: browserProcessTree(current, roots[0]) }
}

export function mergeOwnedProcesses(...snapshots) {
  const identities = new Map()
  for (const rows of snapshots)
    for (const row of rows) identities.set(`${row.pid}:${row.createdUtc}`, row)
  return [...identities.values()]
}

export function stillOwnedProcesses(current, owned) {
  return current.filter((row) =>
    owned.some((previous) => previous.pid === row.pid && previous.createdUtc === row.createdUtc),
  )
}
export async function posixListenerPid(port) {
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Invalid listener port')
  const { stdout } = await execute('lsof', ['-nP', `-iTCP:${port}`, '-sTCP:LISTEN', '-t'])
  const ids = [...new Set(stdout.trim().split(/\s+/).map(Number))]
  if (ids.length !== 1 || !Number.isInteger(ids[0]) || ids[0] < 1)
    throw new Error('Expected one owned backend listener')
  return ids[0]
}
