// SSH 密码登录时“这个提示该不该给密码”的规则，以及助手和 Runtime 之间约定的几个名字。
// 判断在 Runtime 这边做（见 ssh-askpass-channel.mjs）：助手（runtime/workers/ssh-askpass.mjs）只把 ssh 的提示语
// 原样转过来。环境变量名和标记在助手里另有一份（它是单独启动的进程，只依赖 Node 内建模块），测试守着两份一致。

export const ASKPASS_FLAG = 'PISPER_SSH_ASKPASS'
// 向 Runtime 要密码的本地通道（unix socket 或 Windows 命名管道）和只能用一次的令牌。
export const ASKPASS_CHANNEL = 'PISPER_SSH_ASKPASS_CHANNEL'
export const ASKPASS_TOKEN = 'PISPER_SSH_ASKPASS_TOKEN'
export const ASKPASS_VARIABLES = [ASKPASS_FLAG, ASKPASS_CHANNEL, ASKPASS_TOKEN]
// 没给密码时助手写到 stderr 的标记：ssh 把它原样转给 Pisper。问的不是密码（要第二重验证，不支持）；
// 问密码的不是目标机（密码没有交出去）。
export const ASKPASS_REFUSED = 'PISPER_SSH_ASKPASS_REFUSED'
export const ASKPASS_OTHER_HOST = 'PISPER_SSH_ASKPASS_OTHER_HOST'

// 只有这几种写法算“要密码”：Password: / Password for me: / 密码：
const PASSWORD_PROMPT = /^(?:password(?: for [^\s:]{1,255})?|密码)\s*[:：]\s*$/i
// ssh 自己拼的密码提示：user@host's password:
const CLIENT_PROMPT = /^(.{1,255})@([^@]{1,255})'s password:\s*$/i
// 键盘交互方式下 ssh（8.5 起）在服务器的提示前加上 (user@host)。
const PREFIXED_PROMPT = /^\((.{1,255})@([^@()]{1,255})\)\s*(.*)$/s

function expected(value) {
  return (Array.isArray(value) ? value : String(value ?? '').split(','))
    .map((item) => String(item).trim().toLowerCase())
    .filter(Boolean)
}

/**
 * 这个提示该不该答：'answer' 可以给密码；'other_host' 问的不是目标机；'second_factor' 问的不是密码。
 * @param {unknown} prompt
 * @param {{ expect?: string[] | string, bare?: boolean }} [options]
 * @returns {'answer' | 'other_host' | 'second_factor'}
 */
export function askpassDecision(prompt, { expect = [], bare = false } = {}) {
  const text = String(prompt ?? '').trim()
  const known = expected(expect)
  const isTarget = (user, host) => known.includes(`${user}@${host}`.toLowerCase())
  const client = CLIENT_PROMPT.exec(text)
  if (client) return isTarget(client[1], client[2]) ? 'answer' : 'other_host'
  const prefixed = PREFIXED_PROMPT.exec(text)
  if (prefixed) {
    if (!isTarget(prefixed[1], prefixed[2])) return 'other_host'
    return PASSWORD_PROMPT.test(prefixed[3].trim()) ? 'answer' : 'second_factor'
  }
  if (!PASSWORD_PROMPT.test(text)) return 'second_factor'
  return bare ? 'answer' : 'other_host'
}

/** @param {unknown} prompt @param {unknown} secret @param {{ expect?: string[] | string, bare?: boolean }} [options] */
export function askpassAnswer(prompt, secret, options) {
  const value = typeof secret === 'string' ? secret : ''
  if (!value || askpassDecision(prompt, options) !== 'answer') return null
  return `${value}\n`
}
