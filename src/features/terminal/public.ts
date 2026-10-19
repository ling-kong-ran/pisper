// @public 明确的跨领域入口，公开范围见下方导出。
// 应用壳与任务工作区的轻量公共终端接口；重型视图从 public-views 按需加载。
export { TerminalProvider } from './TerminalProvider'
export { useTerminalService, useTerminals } from './terminal-context'
export type { TerminalRecord } from './terminal-registry'
