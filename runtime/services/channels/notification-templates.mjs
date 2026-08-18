// 通知模板：各通知事件（对话完成/等待确认/定时任务等）的默认模板、变量表与渲染，
// 支持按平台（浏览器/飞书/微信）自定义。
export const NOTIFICATION_EVENTS = {
  'chat.completed': {
    name: '对话完成',
    description: 'Agent 完成回复后发送',
    variables: ['chat.title', 'chat.summary', 'chat.model'],
    defaultContent: '💬 对话「{{chat.title}}」已完成\n\n{{chat.summary}}\n\n模型：{{chat.model}}',
  },
  'chat.waiting': {
    name: '等待用户确认',
    description: 'Agent 暂停运行并等待用户确认时发送',
    variables: ['chat.title', 'chat.tool', 'chat.reason', 'chat.model'],
    defaultContent:
      '⏸️ 对话「{{chat.title}}」正在等待你的确认\n\n操作：{{chat.tool}}\n原因：{{chat.reason}}\n\n模型：{{chat.model}}',
  },
  'schedule.completed': {
    name: '定时任务完成',
    description: '定时任务正常完成后发送',
    variables: ['task.name', 'task.summary', 'task.duration', 'task.nextRun'],
    defaultContent:
      '✅ 定时任务「{{task.name}}」已完成\n\n{{task.summary}}\n\n耗时：{{task.duration}}\n下次运行：{{task.nextRun}}',
  },
  'schedule.failed': {
    name: '定时任务失败',
    description: '定时任务执行失败后发送',
    variables: ['task.name', 'task.error', 'task.duration', 'task.nextRun'],
    defaultContent:
      '❌ 定时任务「{{task.name}}」执行失败\n\n错误：{{task.error}}\n耗时：{{task.duration}}\n下次运行：{{task.nextRun}}',
  },
  'workflow.completed': {
    name: '工作流完成',
    description: '工作流所有节点执行完成后发送',
    variables: ['workflow.name', 'workflow.summary', 'workflow.duration', 'workflow.runId'],
    defaultContent:
      '✅ 工作流「{{workflow.name}}」已完成\n\n{{workflow.summary}}\n\n耗时：{{workflow.duration}}\n运行 ID：{{workflow.runId}}',
  },
  'workflow.failed': {
    name: '工作流失败',
    description: '工作流中断或节点失败后发送',
    variables: ['workflow.name', 'workflow.node', 'workflow.error', 'workflow.runId'],
    defaultContent:
      '❌ 工作流「{{workflow.name}}」执行失败\n\n节点：{{workflow.node}}\n错误：{{workflow.error}}\n运行 ID：{{workflow.runId}}',
  },
}

const SAMPLE_DATA = {
  chat: {
    title: '修复渠道通知',
    summary: '实现已完成，测试和构建均已通过。',
    tool: 'bash',
    reason: '需要确认后才能执行此操作。',
    model: 'openai/gpt-5.4',
  },
  task: {
    name: '每日代码巡检',
    summary: '发现 2 个待处理问题，报告已归档。',
    duration: '2 分 18 秒',
    nextRun: '明天 09:00',
    error: '测试进程超时',
  },
  workflow: {
    name: '发布前检查',
    summary: '测试、构建和安全检查均已通过。',
    duration: '6 分 42 秒',
    runId: 'run_20260718_001',
    node: '端到端测试',
    error: '浏览器启动失败',
  },
}

function readPath(value, path) {
  return path.split('.').reduce((current, key) => current?.[key], value)
}

export function renderNotificationTemplate(content, data) {
  return String(content || '').replace(/\{\{\s*([\w.]+)\s*\}\}/g, (_match, path) => {
    const value = readPath(data, path)
    return value == null ? `{{${path}}}` : String(value)
  })
}

export function defaultTemplates() {
  return Object.fromEntries(
    Object.entries(NOTIFICATION_EVENTS).map(([event, definition]) => [
      event,
      {
        enabled: true,
        channels: {
          feishu: { content: definition.defaultContent },
          weixin: { content: definition.defaultContent },
          browser: { content: definition.defaultContent },
        },
      },
    ]),
  )
}

export function normalizeTemplates(input) {
  const defaults = defaultTemplates()
  for (const event of Object.keys(defaults)) {
    const stored = input?.[event]
    if (!stored || typeof stored !== 'object') continue
    defaults[event].enabled = stored.enabled !== false
    for (const platform of ['feishu', 'weixin', 'browser']) {
      const variant = stored.channels?.[platform]
      if (!variant || typeof variant !== 'object') continue
      if (String(variant.content || '').trim())
        defaults[event].channels[platform].content = String(variant.content).slice(0, 12_000)
    }
  }
  return defaults
}

export function templateCatalog(templates) {
  return Object.entries(NOTIFICATION_EVENTS).map(([id, definition]) => ({
    id,
    ...definition,
    ...templates[id],
    sample: SAMPLE_DATA,
  }))
}

export function sampleNotificationData() {
  return JSON.parse(JSON.stringify(SAMPLE_DATA))
}
