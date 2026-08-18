// 工作流编辑器向壳层暴露的动作：保存/运行及忙碌状态，
// 供页头按钮与全局快捷键（Cmd+N 主操作）统一触发。
export type WorkflowActions = {
  busy: boolean
  running: boolean
  save: () => void | Promise<unknown>
  run: () => void | Promise<unknown>
}
