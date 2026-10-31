// 工作流编辑器向应用壳报告的状态：只剩“新建 / 编辑”，供手机顶栏的页名使用。
// 保存、试运行、发布等按钮在编辑器自己的 52px 页头里（规格 A6 第 3 批），不再经壳层转发。
export type WorkflowActions = {
  // 新建与编辑共用 /workflows/:id 路由；只有已存在的工作流才算编辑，标题据此区分。
  editing: boolean
}
