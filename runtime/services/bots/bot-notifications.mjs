// 发回来源渠道的 Bot 消息（B5）：从聊天里对 Bot 说的话，回复与中途的审批、提问回到同一个聊天。纯函数，便于测试。
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
