// SSH 密码登录的 askpass 助手。系统的 ssh 不接受命令行密码（这样做 ps 就能看到），只会调用 SSH_ASKPASS
// 指向的程序、把提示语作为参数传进去、从它的 stdout 读密码。SSH_ASKPASS 指向的就是 Pisper 自己的可执行文件：
// 打包版（SEA）由 sea-bootstrap.cjs 看到 PISPER_SSH_ASKPASS=1 后加载本文件；开发版是 node，经 NODE_OPTIONS --import
// 先加载本文件。这样不新增二进制，也不用 .cmd / shell 脚本——提示语来自对方服务器，交给 cmd.exe 解析会被注入命令。
// 密码只经环境变量交给这一个进程，不进命令行、不写盘；这里不打印到 stderr，也不写日志。
// 只依赖 Node 内建模块：打包时原样复制，不进 bundle。
import { writeSync } from 'node:fs'
import { sep } from 'node:path'

export const ASKPASS_FLAG = 'PISPER_SSH_ASKPASS'
export const ASKPASS_SECRET = 'PISPER_SSH_ASKPASS_SECRET'
// 拒绝回答时写到 stderr 的标记：ssh 把它原样转给 Pisper，据此报“需要第二重验证，不支持”。
export const ASKPASS_REFUSED = 'PISPER_SSH_ASKPASS_REFUSED'

// 只回答要密码的提示。验证码、一次性口令、令牌、确认指纹这类不是密码，不能把密码交出去，而是报不支持。
const NOT_A_PASSWORD =
  /verification|one[- ]?time|\botp\b|token|passcode|authenticator|\bcode\b|\bpin\b|duo|yubi|\(yes\/no|fingerprint/i

/** @param {unknown} prompt @param {unknown} secret */
export function askpassAnswer(prompt, secret) {
  const text = String(prompt ?? '')
  const value = typeof secret === 'string' ? secret : ''
  if (!value || NOT_A_PASSWORD.test(text)) return null
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

if (process.env[ASKPASS_FLAG] === '1') {
  const answer = askpassAnswer(askpassPrompt(), process.env[ASKPASS_SECRET])
  // writeSync：Windows 和 macOS 上管道写入是异步的，直接 exit 会丢掉还没写出的密码。
  if (answer === null) {
    writeSync(2, `${ASKPASS_REFUSED}\n`)
    process.exit(1)
  }
  writeSync(1, answer)
  process.exit(0)
}
