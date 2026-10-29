// 工作流编辑器向壳层暴露的动作：保存/运行及忙碌状态，
// 供页头按钮与全局快捷键（Cmd+N 主操作）统一触发。
export type WorkflowActions = {
  busy: boolean
  running: boolean
  save: () => void | Promise<unknown>
  run: () => void | Promise<unknown>
  // 新建与编辑共用 /workflows/:id 路由；只有已存在的工作流才算编辑，页头据此区分标题。
  editing: boolean
}
