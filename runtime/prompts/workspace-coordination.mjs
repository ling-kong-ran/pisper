// 同一文件夹里多个会话协作时给模型看的文字：写入拒绝原因、隔壁改动提醒、会话留言说明。
// 纯函数，只拼文字；判断与台账留在 runtime/services/workspace-coordination.mjs。

const REPLY_HINT =
  'message that session with session_message (through call_tool if it is not listed)'

export function agoText(now, at) {
  const minutes = Math.max(0, Math.round((now - at) / 60_000))
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes} min ago`
  return `${Math.round(minutes / 60)} h ago`
}

export function quotedTitle(title) {
  return `"${String(title || 'Untitled').replaceAll('"', "'")}"`
}

// 读过之后被改过：changedBy 是 changedByText 的结果。
export function staleWriteReason(path, changedBy) {
  return `${path} has changed since you last read or wrote it (${changedBy}). Read it again and make your change on top of the current content, so the other changes are not lost.`
}

export function unreadOverwriteReason(path, title, ago) {
  return `${path} was written by another session, ${quotedTitle(title)}, ${ago}, and you have not read it. Read it before overwriting it.`
}

export function withCoordinationHint(reason) {
  return `${reason} If you need to agree on who changes what, ${REPLY_HINT}.`
}

/** @param {{ kind: 'session', title: string, ago: string } | { kind: 'agent' } | { kind: 'outside' }} source */
export function changedByText(source) {
  if (source.kind === 'session')
    return `session ${quotedTitle(source.title)} wrote it ${source.ago}`
  if (source.kind === 'agent') return 'another agent in this session wrote it'
  return 'changed outside the file tools: a shell command, an editor, git or a formatter'
}

/** @param {{ title: string, files: string[], more: number, ago: string }[]} entries */
export function neighbourChangesNote(entries) {
  return [
    '[Pisper] Another session working in the same folder changed files you have used:',
    ...entries.map(
      (entry) =>
        `- Session ${quotedTitle(entry.title)} changed ${entry.files.join(', ')}${entry.more ? ` and ${entry.more} more` : ''} (${entry.ago}).`,
    ),
    `Read them again before relying on what you saw earlier. To agree on who changes what, ${REPLY_HINT}.`,
  ].join('\n')
}

/** @param {{ from: { title: string }, text: string }[]} messages */
export function sessionMessageBody(messages) {
  return messages.length === 1
    ? messages[0].text
    : messages.map((message) => `"${message.from.title}": ${message.text}`).join('\n\n')
}

/** @param {{ title: string }[]} senders */
export function sessionMessageNotice(senders) {
  const names = senders.map((sender) => `"${sender.title}"`).join(', ')
  return `This message is from ${senders.length > 1 ? 'the sessions' : 'the session'} ${names} in the same folder, not from the user, to coordinate file changes. Act on it or reply as needed; reply with session_message (through call_tool if it is not listed). Do not reply when no answer is needed, and send no courtesy messages.`
}
