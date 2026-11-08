// 发回聊天软件的 Bot 文字（B5）：从聊天里对 Bot 说的话，回复与中途的审批、提问回到同一个聊天；指令清单与拒绝的回话。
// 聊天里的人直接看到、不经过前端翻译，所以保留中文；给模型看的文字不放这里。纯函数，便于测试。
// 回复命令（/名字 批准 等）只有这个 Bot 名单里的人能用：分派时已按名单校验。
import { approvalArgsPreview } from '../session-card-summary.mjs'

// 聊天软件单条消息都有长度上限（微信、QQ 约几千字）：回复太长时截断并提示去 Pisper 里看全文。
const MAX_REPLY_CHARS = 3_000
const MAX_ARGS_CHARS = 400

function clip(value, max) {
  const text = String(value ?? '').trim()
  return text.length > max ? `${text.slice(0, max)}…` : text
}

// 这一轮说完了：发回 Bot 最后说的话；出错时说清楚没做成。
export function botReplyText(name, { text, error }) {
  if (error) return `${name} 没做成：${clip(error, 300)}`
  const reply = String(text ?? '').trim()
  if (!reply) return `${name} 做完了。`
  return reply.length > MAX_REPLY_CHARS
    ? `${reply.slice(0, MAX_REPLY_CHARS)}…\n（太长了，完整回复在 Pisper 里 ${name} 的对话中。）`
    : reply
}

// 等审批：普通操作可以在聊天里批准；高风险操作只能在 Pisper 里看完再批准（任何入口都不能一步批准）。
export function botApprovalText(name, approval) {
  const preview = clip(approvalArgsPreview(approval.toolName, approval.args).text, MAX_ARGS_CHARS)
  const lines = [
    `${name} 需要批准：${approval.toolName || '工具'}`,
    preview,
    approval.reason ? clip(approval.reason, 200) : '',
  ]
  lines.push(
    approval.severity === 'critical'
      ? '这是高风险操作，请在 Pisper 里看完再批准。'
      : `回复 /${name} 批准 或 /${name} 拒绝。`,
  )
  return lines.filter(Boolean).join('\n')
}

// 等回答：问题与选项带出来，回答在 Pisper 里完成（与普通渠道会话一致）。
export function botQuestionText(name, question) {
  const lead = question.questions?.[0]
  if (!lead) return `${name} 有问题问你，请在 Pisper 里回答。`
  const options = (lead.options || []).map((option) => `· ${option.label}`).join('\n')
  return [`${name} 问：${clip(lead.question, 300)}`, options, '请在 Pisper 里回答。']
    .filter(Boolean)
    .join('\n')
}

// 聊天里对 Bot 说的话是不是处理命令（整句只有这个词才算）。
export function botChatAction(text) {
  const value = String(text ?? '')
    .trim()
    .toLowerCase()
  if (/^(?:批准|同意|approve|yes)$/.test(value)) return 'approve'
  if (/^(?:拒绝|deny|no)$/.test(value)) return 'deny'
  return null
}

// 指令清单：只列这个人能用的，管理员另外提示可以直接说要做的事。不调用模型。
// 群里不是每个平台都投递不带 @ 的消息（QQ、飞书只投递 @ 了机器人的），所以群里提示先 @ 机器人。
export function botMenuReply(name, commands, { admin = false, group = false } = {}) {
  if (!commands.length)
    return admin ? `要让 ${name} 做什么？在名字后面写上。` : `${name} 在这里还没有开放的指令。`
  return [
    group ? `${name} 可以做这些，@ 机器人后写序号即可：` : `${name} 可以做这些，回复序号即可：`,
    ...commands.map((command, index) => `${index + 1}. ${command}`),
    admin ? '也可以在名字后面直接写要做的事。' : '',
  ]
    .filter(Boolean)
    .join('\n')
}

// 叫不动时的回话：已暂停、不在名单里、只有管理员能做、不在这个聊天里。
export function botRefusalReply(name, reason) {
  if (reason === 'paused') return `${name} 已暂停，不接新任务。`
  if (reason === 'sender_not_allowed') return `你不在 ${name} 的名单里，叫不动它。`
  if (reason === 'admin_only') return `这件事只有 ${name} 的管理员能让它做。`
  return `${name} 不在这个聊天里。`
}

// Bot 用 bot_notify 发到聊天应用的消息：前面带上 Bot 名字，聊天里看得出是谁说的。
export function botNotifyText(name, message) {
  return `${name}：${message}`
}

// 聊天里“/名字 批准 | 拒绝”的回复；outcome 由运行时判定。
export function botChannelActionReply(name, outcome) {
  if (outcome === 'no_bot') return 'Bot 不存在。'
  if (outcome === 'nothing') return `${name} 现在没有等批准的操作。`
  if (outcome === 'critical') return '这是高风险操作，请在 Pisper 里看完再批准。'
  if (outcome === 'handled') return '这个审批已经处理过了。'
  return outcome === 'approved' ? `已批准，${name} 继续干活。` : '已拒绝这次操作。'
}
