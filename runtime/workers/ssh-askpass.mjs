// SSH 密码登录的 askpass 助手。系统的 ssh 不接受命令行密码（这样做 ps 就能看到），只会调用 SSH_ASKPASS
// 指向的程序、把提示语作为参数传进去、从它的 stdout 读密码。SSH_ASKPASS 指向的就是 Pisper 自己的可执行文件：
// 打包版（SEA）由 sea-bootstrap.cjs 看到 PISPER_SSH_ASKPASS=1 后加载本文件；开发版是 node，经 NODE_OPTIONS --import
// 先加载本文件。这样不新增二进制，也不用 .cmd / shell 脚本——提示语来自对方服务器，交给 cmd.exe 解析会被注入命令。
// 密码只经环境变量交给这一个进程，不进命令行、不写盘；这里不打印到 stderr，也不写日志。
//
// 密码只答给它该去的那台机器，而且只答一次：
// - 提示语里写着是谁在问（user@host's password: / (user@host) Password:）：必须是 Pisper 事先用 ssh -G 解析出的
//   那台目标机，否则不答。跳板机（ProxyJump）的内层 ssh 会继承同一份环境，它问密码时不能把目标机的密码交出去。
// - 只认“要密码”的提示（白名单）。验证码、一次性口令、确认指纹等一律不答，报“不支持第二重验证”。
// - 一次连接只答一次（一次性令牌文件）：密码错了，ssh 换一种方式再问时不再送第二遍。
// - 开发版的 NODE_OPTIONS 会被 ssh 的子进程继承（ProxyCommand 里的 node 程序）：不是被当作 askpass 调用时
//   什么都不做，并把这几个变量从自己的环境里拿掉，不再往下传。
// 只依赖 Node 内建模块：打包时原样复制，不进 bundle。
import { existsSync, unlinkSync, writeSync } from 'node:fs'
import { sep } from 'node:path'
import { isSea } from 'node:sea'

export const ASKPASS_FLAG = 'PISPER_SSH_ASKPASS'
export const ASKPASS_SECRET = 'PISPER_SSH_ASKPASS_SECRET'
// 目标机的身份，逗号分隔的 user@host（解析后的主机名，以及 HostKeyAlias）。
export const ASKPASS_EXPECT = 'PISPER_SSH_ASKPASS_EXPECT'
// 这次连接不经过跳板或代理命令：没写是谁在问的“Password:”只可能来自目标机，可以答。
export const ASKPASS_BARE = 'PISPER_SSH_ASKPASS_BARE'
// 一次性令牌文件：删掉它的那一次才回答。
export const ASKPASS_ONCE = 'PISPER_SSH_ASKPASS_ONCE'
export const ASKPASS_VARIABLES = [
  ASKPASS_FLAG,
  ASKPASS_SECRET,
  ASKPASS_EXPECT,
  ASKPASS_BARE,
  ASKPASS_ONCE,
]
// 拒绝回答时写到 stderr 的标记：ssh 把它原样转给 Pisper，据此报“需要第二重验证，不支持”。
export const ASKPASS_REFUSED = 'PISPER_SSH_ASKPASS_REFUSED'
// 问密码的不是目标机（跳板机，或认不出是谁）：密码没有交出去。
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

// node 会把第一个参数当脚本路径解析成绝对路径：去掉当前目录前缀，还原提示语。
export function askpassPrompt(
  argv = process.argv,
  cwd = process.cwd(),
  execPath = process.execPath,
) {
  const last = argv.length > 1 ? String(argv.at(-1)) : ''
  if (!last || last === execPath) return ''
  return last.startsWith(`${cwd}${sep}`) ? last.slice(cwd.length + 1) : last
}

// 是不是 ssh 在把这个进程当 askpass 调用。打包版只有入口脚本认出标记才会加载到这里；开发版是 node：
// ssh 只传一个参数（提示语），node 把它当脚本路径——那不会是一个存在的文件。带着真实脚本启动的，
// 是继承了环境的别的 node 程序。
export function invokedAsAskpass(
  argv = process.argv,
  { packaged = isSea(), exists = existsSync } = {},
) {
  if (packaged) return true
  return argv.length === 2 && !exists(String(argv[1]))
}

// 一次性令牌：谁删掉这个文件谁回答；已经没有了就是答过了。没配令牌时不限制（只有测试会这样调）。
export function consumeAskpassToken(path, remove = unlinkSync) {
  if (!path) return true
  try {
    remove(path)
    return true
  } catch {
    return false
  }
}

if (process.env[ASKPASS_FLAG] === '1') {
  if (!invokedAsAskpass()) {
    // 不是 askpass 调用：不回答，也不让密码继续传给这个程序的子进程。
    for (const name of ASKPASS_VARIABLES) delete process.env[name]
    delete process.env.NODE_OPTIONS
  } else {
    const decision = askpassDecision(askpassPrompt(), {
      expect: process.env[ASKPASS_EXPECT],
      bare: process.env[ASKPASS_BARE] === '1',
    })
    const secret = process.env[ASKPASS_SECRET]
    if (decision !== 'answer' || !secret) {
      writeSync(2, `${decision === 'other_host' ? ASKPASS_OTHER_HOST : ASKPASS_REFUSED}\n`)
      process.exit(1)
    }
    // 已经答过一次：不再送，也不留标记——接下来 ssh 自己的“Permission denied”就是结论（密码不对）。
    if (!consumeAskpassToken(process.env[ASKPASS_ONCE])) process.exit(1)
    // writeSync：Windows 和 macOS 上管道写入是异步的，直接 exit 会丢掉还没写出的密码。
    writeSync(1, `${secret}\n`)
    process.exit(0)
  }
}
