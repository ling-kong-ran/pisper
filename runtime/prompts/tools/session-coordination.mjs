// list_sessions / session_message 的说明与结果文字（同一文件夹里会话之间的协调）。
export const LIST_SESSIONS = Object.freeze({
  description:
    'List the other sessions (including bots) in the same folder: whether each is running and which files it changed in the last 30 minutes.',
})

export const SESSION_MESSAGE = Object.freeze({
  description:
    'Send a short message to another session in the same folder to coordinate changes, e.g. who edits which files. A running session reads it at its next step; an idle one when the user next works in it (it is never started for you). No courtesy messages.',
  params: { session: 'The session’s title or id, as list_sessions shows it.' },
  /** @param {string} title */
  sentNow: (title) => `Sent to ${title}; it reads it at its next step.`,
  /** @param {string} title */
  left: (title) =>
    `Left for ${title}; it is idle and gets the message when the user next works in it.`,
})
