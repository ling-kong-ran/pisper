// 无项目会话：新任务不选项目时，在数据目录下给每个会话建一个独立的空文件夹作为工作目录。
// 代理写出的文件都落在这里，不会碰到用户主目录或某个项目；是否“无项目”只按路径判断，
// 不另存标记，复刻、恢复和移动后的会话都能得到一致结果。
import { mkdir, mkdtemp } from 'node:fs/promises'
import { join } from 'node:path'
import { workspacePathKey } from './workspace-directories.mjs'

export function projectlessChatsRoot(dataDir) {
  return join(dataDir, 'chats')
}

// 文件夹名带日期，方便用户在文件管理器里按时间找；随机后缀保证并发新建互不冲突。
export async function createProjectlessDirectory(root, now = new Date()) {
  await mkdir(root, { recursive: true, mode: 0o700 })
  return mkdtemp(join(root, `${now.toISOString().slice(0, 10)}-`))
}

// 只认 root 下的直接子目录：root 本身或更深的子目录都按普通项目处理。
export function isProjectlessDirectory(root, cwd, platform = process.platform) {
  if (!root || !cwd) return false
  const base = workspacePathKey(root, platform).replaceAll('\\', '/')
  const path = workspacePathKey(cwd, platform).replaceAll('\\', '/')
  if (!path.startsWith(`${base}/`)) return false
  const rest = path.slice(base.length + 1)
  return rest.length > 0 && !rest.includes('/') && rest !== '..' && rest !== '.'
}
