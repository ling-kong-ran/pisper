// 会话导出为 Markdown（A9，标题菜单"导出"）：只读已经投影好的转录（serializeTranscriptMessages 的结果），
// 不读引擎原始文件。导出内容：标题、导出时间、项目名（只写目录名，不写完整个人路径）、模型，
// 然后按回合列出用户消息与代理回复；代理用过的工具列成一行摘要（命令、路径），图片附件只写占位，
// 思考过程不导出（可能很长，也可能包含不想外传的推理）。
const LABELS = {
  'zh-CN': {
    exportedAt: '导出时间',
    project: '项目',
    model: '模型',
    user: '你',
    agent: '代理',
    tools: (count) => `${count} 个操作`,
    image: (index) => `[图片附件 ${index}]`,
    error: '错误',
    empty: '（这个会话还没有消息）',
  },
  'en-US': {
    exportedAt: 'Exported',
    project: 'Project',
    model: 'Model',
    user: 'You',
    agent: 'Agent',
    tools: (count) => `${count} ${count === 1 ? 'action' : 'actions'}`,
    image: (index) => `[Image attachment ${index}]`,
    error: 'Error',
    empty: '(No messages in this session yet)',
  },
}

const MAX_ARG_SUMMARY = 160

function oneLine(value, limit = MAX_ARG_SUMMARY) {
  const text = String(value ?? '')
    .replace(/\s+/g, ' ')
    .trim()
  const points = Array.from(text)
  return points.length > limit ? `${points.slice(0, limit - 1).join('')}…` : text
}

// 行内代码：用比内容里最长的连续反引号多一个的反引号包起来，内容里有反引号也不会断开。
function inlineCode(value) {
  const text = oneLine(value)
  if (!text) return ''
  const longest = Math.max(0, ...(text.match(/`+/g) || []).map((run) => run.length))
  const fence = '`'.repeat(longest + 1)
  const pad = text.startsWith('`') || text.endsWith('`') ? ' ' : ''
  return `${fence}${pad}${text}${pad}${fence}`
}

function toolSummary(tool) {
  const args = tool && typeof tool.args === 'object' && tool.args ? tool.args : {}
  const target = args.command ?? args.path ?? args.file_path ?? args.pattern ?? args.query ?? ''
  const name = oneLine(tool?.name || 'tool', 60)
  const detail = target ? ` ${inlineCode(target)}` : ''
  const failed = tool?.status === 'error' ? ' ✕' : ''
  return `- ${name}${detail}${failed}`
}

function formatTime(value, locale) {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  return date.toLocaleString(locale, { hour12: false })
}

function projectName(cwd) {
  const parts = String(cwd || '')
    .split(/[\\/]+/)
    .filter(Boolean)
  return parts.at(-1) || ''
}

/**
 * @param {{
 *   session: { name?: string, cwd?: string, model?: string },
 *   messages: ReadonlyArray<Record<string, any>>,
 *   exportedAt?: Date,
 *   locale?: string,
 * }} input
 */
export function sessionToMarkdown({
  session,
  messages,
  exportedAt = new Date(),
  locale = 'zh-CN',
}) {
  const labels = LABELS[locale] || LABELS['zh-CN']
  const lines = [`# ${oneLine(session?.name || 'Pisper', 200)}`, '']
  lines.push(`- ${labels.exportedAt}: ${formatTime(exportedAt, locale)}`)
  const project = projectName(session?.cwd)
  if (project) lines.push(`- ${labels.project}: ${project}`)
  if (session?.model) lines.push(`- ${labels.model}: ${oneLine(session.model, 120)}`)
  lines.push('')
  const visible = (messages || []).filter(
    (message) => message && (message.role === 'user' || message.role === 'agent'),
  )
  if (!visible.length) {
    lines.push(labels.empty, '')
    return `${lines.join('\n')}`
  }
  for (const message of visible) {
    const time = message.timestamp ? formatTime(message.timestamp, locale) : ''
    const who = message.role === 'user' ? labels.user : labels.agent
    lines.push('---', '', `## ${who}${time ? ` · ${time}` : ''}`, '')
    const tools = Array.isArray(message.runActivity?.tools) ? message.runActivity.tools : []
    if (message.role === 'agent' && tools.length) {
      lines.push(`**${labels.tools(tools.length)}**`, '', ...tools.map(toolSummary), '')
    }
    const text = String(message.text || '').trim()
    if (text) lines.push(text, '')
    const images = Array.isArray(message.attachments)
      ? message.attachments.filter((attachment) => attachment?.kind === 'image')
      : []
    images.forEach((_, index) => lines.push(labels.image(index + 1)))
    if (images.length) lines.push('')
    if (message.error) lines.push(`> ${labels.error}: ${oneLine(message.error, 400)}`, '')
  }
  return `${lines
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trimEnd()}\n`
}

/** 下载文件名：会话名去掉文件系统不允许的字符，限长；空名用 pisper-session。 */
export function exportFileName(name) {
  const base = oneLine(name, 80)
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u001f<>:"/\\|?*]+/g, ' ')
    .replace(/\s+/g, ' ')
    // 去掉首尾的点和空白：避免隐藏文件名，以及 Windows 不接受的结尾点。
    .replace(/^[.\s]+|[.\s]+$/g, '')
  return `${base || 'pisper-session'}.md`
}
