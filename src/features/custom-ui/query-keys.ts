// 原生组件查询键（计划 §4.1）：都在 ['custom-ui'] 前缀下，组件变化时可以按前缀整体作废。
// 旧 iframe 目录的键 ['custom-ui-components'] 不在这个前缀下，S4 随旧机制一起删除。
export const CUSTOM_UI_QUERY_ROOT = ['custom-ui'] as const

export const customUiQueryKeys = {
  all: CUSTOM_UI_QUERY_ROOT,
  catalog: () => ['custom-ui', 'catalog'] as const,
  instances: () => ['custom-ui', 'instances'] as const,
  describe: (id: string) => ['custom-ui', 'describe', id] as const,
  errors: (id: string) => ['custom-ui', 'errors', id] as const,
}
