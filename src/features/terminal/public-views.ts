// @public 明确的跨领域入口，公开范围见下方导出。
// 重型视图通过独立 chunk 加载，不让进程列表拉入 xterm。
import { lazy } from 'react'
export const TaskTerminalView = lazy(() =>
  import('./TaskTerminalView').then((module) => ({ default: module.TaskTerminalView })),
)
export const TaskProcessesPane = lazy(() =>
  import('./TaskProcessesPane').then((module) => ({ default: module.TaskProcessesPane })),
)
