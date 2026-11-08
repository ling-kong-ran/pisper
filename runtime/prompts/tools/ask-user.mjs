// ask_user 的说明与结果文字。题目格式（ASK_USER_QUESTION）也给 Bot 对话的 bot_ask 用。

export const ASK_USER_QUESTION = Object.freeze({
  optionLabel: 'Button text (≤60 chars)',
  optionDescription: 'One-line explanation (≤200 chars)',
  header: 'Short title (≤24 chars)',
  question: 'Full question (≤400 chars)',
  options: 'Distinct choices. No "Other" option: the user can type an answer.',
})

export const ASK_USER = Object.freeze({
  description:
    'Ask the user 1–4 multiple-choice questions and wait for the answers. They can also type an answer or skip.',
  guidelines: [
    'Use ask_user only when you cannot proceed well without the user choosing between concrete alternatives (requirements, trade-offs, destructive choices), not for tool permission. Put related questions in one call and the recommended option first. If the user skips, decide on your own.',
  ],
  unavailable:
    'ask_user is not available in this run because no user is present to answer. Decide on your own.',
  skipped:
    'The user skipped these questions. Decide on your own, or ask again differently if the decision truly needs them.',
  /** @param {string} [reason] */
  cancelled: (reason) =>
    `The questions were cancelled before the user answered (${reason || 'run stopped'}). Do not assume any answer.`,
})
