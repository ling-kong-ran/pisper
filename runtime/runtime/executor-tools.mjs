// 远程会话的工具（B6）：Bot 选了执行节点时，它的会话里只有能交给那台电脑执行的工具。
// 读、改、写、列目录、找文件、跑命令都换成执行节点的 operations；联网搜索在大脑上运行，与那台电脑无关。
// 其余工具（grep 依赖本机 ripgrep、浏览器、计算机操作、MCP、插件、子代理、记忆等）只能在大脑本机用，
// 远程会话里不提供，也在权限层拦下（runToolBlocker 读 executorToolNames），不假装可用。
import {
  createBashTool,
  createEditTool,
  createFindTool,
  createLsTool,
  createReadTool,
  createWriteTool,
} from './pi-coding-agent.mjs'
import { posix, win32 } from 'node:path'
import { leanToolDefinition } from '../tools/lean-coding-tools.mjs'

export const REMOTE_EXECUTOR_TOOL_NAMES = Object.freeze([
  'read',
  'edit',
  'write',
  'ls',
  'find',
  'bash',
  'web_search',
])

// 会话元数据里的 remoteExecutor（不可信）：节点 id 与那台电脑上的绝对路径。
export function normalizeRemoteExecutor(value) {
  if (!value || typeof value !== 'object') return null
  const environmentId = String(value.environmentId || '')
  const cwd = String(value.cwd || '')
  if (!/^(?:env|ssh)_[A-Za-z0-9_-]{8,32}$/.test(environmentId)) return null
  if (!cwd || cwd.length > 1024 || cwd.includes('\0') || !/^(?:\/|[A-Za-z]:[\\/])/.test(cwd))
    return null
  return { environmentId, cwd }
}

// 普通会话选了远程机器时告诉模型：路径和命令都在那台机器上，不要按用户自己的电脑去想。
export function machineSystemPrompt(machine, cwd) {
  return [
    `You are working on the remote machine "${machine.name}", not on the user's own computer.`,
    `File and shell tools run on that machine, in ${cwd}. All paths are paths on that machine.`,
    'Tools that are not listed are not available here.',
  ].join('\n')
}

// Pi 的工具按大脑本机的路径规则解析路径：Windows 大脑会把远程 Linux 的 /repo/a 解析成 E:\repo\a，
// Linux 大脑会把远程 Windows 的 C:\repo 当成相对路径接在自己的目录后面。
// 交给执行节点之前换回远程机器的写法。
const isWindowsPath = (path) => /^[A-Za-z]:[\\/]/.test(path)
export function toRemotePath(path, remoteCwd, hostPlatform = process.platform) {
  const value = String(path)
  const remoteWindows = isWindowsPath(remoteCwd)
  if (remoteWindows === (hostPlatform === 'win32')) return value
  if (!remoteWindows) return isWindowsPath(value) ? value.slice(2).replaceAll('\\', '/') : value
  // 远程是 Windows：路径已先按 Windows 规则解析成绝对路径（resolveRemoteParam），
  // 本机再接上自己的目录，最后一个盘符起就是原路径。
  const index = [...value.matchAll(/[A-Za-z]:[\\/]/g)].at(-1)?.index ?? -1
  return index > 0 ? win32.normalize(value.slice(index)) : value
}

// 按远程机器的路径规则把工具参数里的路径解析成绝对路径，本机解析时就不会再改动它。
function resolveRemoteParam(value, remoteCwd) {
  if (typeof value !== 'string' || !value || value.startsWith('~')) return value
  const flavor = isWindowsPath(remoteCwd) ? win32 : posix
  return flavor.resolve(remoteCwd, value.startsWith('@') ? value.slice(1) : value)
}

// 每个操作的哪个参数是路径：find.glob 的第一个参数是模式，写文件的第二个参数是内容。
const PATH_ARGUMENT = { glob: 1, exec: 1 }
function remoteOperations(operations, remoteCwd, hostPlatform) {
  return Object.fromEntries(
    Object.entries(operations).map(([name, operation]) => [
      name,
      typeof operation !== 'function'
        ? operation
        : (...args) => {
            const at = PATH_ARGUMENT[name] ?? 0
            if (typeof args[at] === 'string')
              args[at] = toRemotePath(args[at], remoteCwd, hostPlatform)
            return operation(...args)
          },
    ]),
  )
}

export async function createExecutorTools(
  cwd,
  executor,
  enabledTools,
  { hostPlatform = process.platform } = {},
) {
  const operations = executor.operations()
  const remote = (name) => ({ operations: remoteOperations(operations[name], cwd, hostPlatform) })
  const enabled = new Set(enabledTools)
  const tools = [
    await createReadTool(cwd, remote('read')),
    await createEditTool(cwd, remote('edit')),
    await createWriteTool(cwd, remote('write')),
    await createLsTool(cwd, remote('ls')),
    await createFindTool(cwd, remote('find')),
  ]
  if (enabled.has('bash') && executor.capabilities.shell)
    tools.push(await createBashTool(cwd, remote('bash')))
  return tools
    .filter((tool) => enabled.has(tool.name))
    .map((tool) => {
      const execute = tool.execute
      return leanToolDefinition({
        ...tool,
        execute: (toolCallId, params, ...rest) =>
          execute.call(
            tool,
            toolCallId,
            params && typeof params === 'object' && 'path' in params
              ? { ...params, path: resolveRemoteParam(params.path, cwd) }
              : params,
            ...rest,
          ),
      })
    })
}
