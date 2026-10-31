// “更多”标签的归属常量：外壳（标题栏高亮、页头“‹ 更多”、返回键）常驻需要，单独成模块，
// 不把启动台磁贴的图标带进启动包。磁贴分组本身在 more-navigation.ts。

// 更多页搜索框的 DOM id：外壳的“/ 聚焦搜索”在更多页上落到这里。
export const MORE_SEARCH_INPUT_ID = 'more-page-search'

// 从“更多”进入时归“更多”标签的功能页；插件、星忆等同时挂在设置下的页面按进入来源判定。
export const MORE_PAGES: ReadonlySet<string> = new Set([
  'chatHistory',
  'assets',
  'memory',
  'workflows',
  'workflowCreate',
  'schedules',
  'channels',
  'plugins',
  'skills',
  'mcp',
  'customComponentTool',
])

// 已改用功能页模板（src/components/app/FeaturePage.tsx，规格 A6）的页面：页头、搜索与主操作都在页面自己的
// 滚动区里，应用壳不再渲染共享页头（PageHeader）。A6 后面几批迁移一页就加一页，守卫逐页核对。
export const FEATURE_TEMPLATE_PAGES: ReadonlySet<string> = new Set([
  'plugins',
  'skills',
  'mcp',
  'chatHistory',
  'assets',
  'memory',
])

// 功能页模板里搜索框的 DOM id：外壳的“/ 聚焦搜索”在这些页上落到这里（同一时刻只挂一页）。
export const FEATURE_SEARCH_INPUT_ID = 'feature-page-search'
