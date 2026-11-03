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
  if (!/^env_[A-Za-z0-9_-]{8,32}$/.test(environmentId)) return null
  if (!cwd || cwd.length > 1024 || cwd.includes('\0') || !/^(?:\/|[A-Za-z]:[\\/])/.test(cwd))
    return null
  return { environmentId, cwd }
}

export async function createExecutorTools(cwd, executor, enabledTools) {
  const operations = executor.operations()
  const enabled = new Set(enabledTools)
  const tools = [
    await createReadTool(cwd, { operations: operations.read }),
    await createEditTool(cwd, { operations: operations.edit }),
    await createWriteTool(cwd, { operations: operations.write }),
    await createLsTool(cwd, { operations: operations.ls }),
    await createFindTool(cwd, { operations: operations.find }),
  ]
  if (enabled.has('bash') && executor.capabilities.shell)
    tools.push(await createBashTool(cwd, { operations: operations.bash }))
  return tools.filter((tool) => enabled.has(tool.name))
}
