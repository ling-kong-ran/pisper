// 同一文件夹里会话之间的协调工具：list_sessions 看谁也在这里、最近改了哪些文件；
// session_message 给某个会话留言（对方在跑就插进它的下一步，没在跑就等用户在那边处理，不会替它开一轮）。
// 只是传话：不经审批，也不给对方任何权限。真正的发送、频率上限与目录范围在运行时里校验。
import { Type } from 'typebox'
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { LIST_SESSIONS, SESSION_MESSAGE } from '../../prompts/tools/session-coordination.mjs'

export { SESSION_COORDINATION_TOOL_NAMES } from './session-coordination-tool-names.mjs'

function minutesAgo(now, iso) {
  const at = Date.parse(iso)
  if (!Number.isFinite(at)) return ''
  const minutes = Math.max(0, Math.round((now - at) / 60_000))
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.round(minutes / 60)
  return hours < 48 ? `${hours} h ago` : `${Math.round(hours / 24)} d ago`
}

export function sessionListText(sessions, now = Date.now()) {
  if (!sessions.length) return 'No other session works in this folder.'
  return [
    'Other sessions in this folder, most recently active first:',
    ...sessions.map((item) => {
      const state = item.running ? 'running' : `idle, active ${minutesAgo(now, item.modified)}`
      const files = item.recentFiles.length
        ? `; changed in the last 30 min: ${item.recentFiles.join(', ')}`
        : ''
      return `- "${item.title || 'Untitled'}" (id ${item.id.slice(0, 8)}, ${state}${files})`
    }),
  ].join('\n')
}

/** @param {{ runtime: { list: () => Promise<any[]>, send: (input: any) => Promise<any> } }} options */
export function createSessionCoordinationTools({ runtime }) {
  return [
    defineTool({
      name: 'list_sessions',
      label: 'List sessions in this folder',
      description: LIST_SESSIONS.description,
      parameters: Type.Object({}),
      async execute() {
        const sessions = await runtime.list()
        return {
          content: [{ type: 'text', text: sessionListText(sessions) }],
          details: { sessions },
        }
      },
    }),
    defineTool({
      name: 'session_message',
      label: 'Message another session',
      description: SESSION_MESSAGE.description,
      parameters: Type.Object({
        session: Type.String({
          minLength: 1,
          maxLength: 200,
          description: SESSION_MESSAGE.params.session,
        }),
        text: Type.String({ minLength: 1, maxLength: 4000 }),
      }),
      async execute(_toolCallId, params) {
        const result = await runtime.send(params)
        const title = `"${result.session.title || 'Untitled'}"`
        const text =
          result.delivered === 'now' ? SESSION_MESSAGE.sentNow(title) : SESSION_MESSAGE.left(title)
        return { content: [{ type: 'text', text }], details: result }
      },
    }),
  ]
}
