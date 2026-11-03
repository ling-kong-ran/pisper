// 回推到来源渠道的 Bot 消息（B5）：从聊天里交给 Bot 的事，进展回到同一个聊天。纯函数，便于测试。
// 回复命令（/名字 批准 等）只有这个 Bot 名单里的人能用：分派时已按名单校验。
import { approvalArgsPreview } from '../session-card-summary.mjs'

const MAX_SUMMARY_CHARS = 1_200
const MAX_ARGS_CHARS = 400

function clip(value, max) {
  const text = String(value ?? '').trim()
  return text.length > max ? `${text.slice(0, max)}…` : text
}

// 任务状态变化：待确认、完成、失败、中断。
export function botTaskEventText(name, task, kind) {
  const title = task.title ? `「${clip(task.title, 60)}」` : ''
  const summary = clip(task.result?.summary, MAX_SUMMARY_CHARS)
  switch (kind) {
    case 'review':
      return [
        `${name} 改好了${title}：${task.result?.changedFiles || 0} 个文件有改动${task.workspace?.branch ? `，在分支 ${task.workspace.branch} 上` : ''}。`,
        summary,
        `回复 /${name} 应用（接受并应用到当前分支）、/${name} 接受（只接受，留在任务分支）或 /${name} 放弃；接着说就在同一个分支上继续改。`,
      ]
        .filter(Boolean)
        .join('\n')
    case 'done':
      return [`${name} 做完了${title}。`, summary].filter(Boolean).join('\n')
    case 'failed':
      return `${name} 没做成${title}：${clip(task.error?.message, 300) || '出错了'}`
    case 'interrupted':
      return `${name} 的${title}被中断了（${clip(task.error?.message, 200) || '可能是电脑断线或 Pisper 重启'}）。在 Pisper 里点“继续”或“放弃”。`
    default:
      return ''
  }
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
  if (/^(?:应用|apply)$/.test(value)) return 'apply'
  if (/^(?:接受|accept)$/.test(value)) return 'accept'
  if (/^(?:放弃|discard)$/.test(value)) return 'discard'
  return null
}
