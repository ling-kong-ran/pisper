// SSH 密码登录的 askpass 助手。系统的 ssh 不接受命令行密码（这样做 ps 就能看到），只会调用 SSH_ASKPASS
// 指向的程序、把提示语作为参数传进去、从它的 stdout 读密码。SSH_ASKPASS 指向的就是 Pisper 自己的可执行文件：
// 打包版（SEA）由 sea-bootstrap.cjs 看到 PISPER_SSH_ASKPASS=1 后加载本文件；开发版是 node，经 NODE_OPTIONS --import
// 先加载本文件。这样不新增二进制，也不用 .cmd / shell 脚本——提示语来自对方服务器，交给 cmd.exe 解析会被注入命令。
//
// 助手自己没有密码，也不做判断：环境里只有通道地址和一次性令牌（见 runtime/services/ssh-askpass-channel.mjs）。
// 它把 ssh 给的提示语原样和令牌一起交给 Runtime，由 Runtime 决定给不给（规则在 ssh-askpass-prompt.mjs）：
// - 提示语里写着是谁在问（user@host's password: / (user@host) Password:）：必须是这次要连的那台机器，否则不给。
// - 只认“要密码”的提示（白名单）。验证码、一次性口令、确认指纹等一律不给，报“不支持第二重验证”。
// - 一次连接只给一次：密码错了，ssh 换一种方式再问时不再送第二遍。
// 密码不经过任何环境变量：ssh 的子进程继承这份环境也读不到它。这里不打印到 stderr，也不写日志。
// - 开发版的 NODE_OPTIONS 会被 ssh 的子进程继承：不是被当作 askpass 调用时什么都不做，并把这几个变量从自己的
//   环境里拿掉，不再往下传。
// 只依赖 Node 内建模块：打包时原样复制，不进 bundle。Runtime 不导入这个文件（它带顶层 await）：两边共用的
// 几个名字在 ssh-askpass-prompt.mjs 里另有一份，测试守着两份一致。
import { existsSync, writeSync } from 'node:fs'
import { connect } from 'node:net'
import { sep } from 'node:path'
import { isSea } from 'node:sea'

export const ASKPASS_FLAG = 'PISPER_SSH_ASKPASS'
// 向 Runtime 要密码的本地通道（unix socket 或 Windows 命名管道）和只能用一次的令牌。
export const ASKPASS_CHANNEL = 'PISPER_SSH_ASKPASS_CHANNEL'
export const ASKPASS_TOKEN = 'PISPER_SSH_ASKPASS_TOKEN'
export const ASKPASS_VARIABLES = [ASKPASS_FLAG, ASKPASS_CHANNEL, ASKPASS_TOKEN]
// 拒绝回答时写到 stderr 的标记：ssh 把它原样转给 Pisper，据此报“需要第二重验证，不支持”。
export const ASKPASS_REFUSED = 'PISPER_SSH_ASKPASS_REFUSED'
// 问密码的不是目标机（跳板机，或认不出是谁）：密码没有交出去。
export const ASKPASS_OTHER_HOST = 'PISPER_SSH_ASKPASS_OTHER_HOST'

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

const REQUEST_TIMEOUT_MS = 8_000

/**
 * 向 Runtime 要密码：发一行 JSON（令牌 + 提示语原文），收一行 JSON。连不上、超时、回复看不懂都算没要到。
 * @param {string} address
 * @param {string} token
 * @param {string} prompt
 * @returns {Promise<{ ok: boolean, secret?: string, reason?: string }>}
 */
export function requestAskpassSecret(address, token, prompt, timeoutMs = REQUEST_TIMEOUT_MS) {
  return new Promise((resolve) => {
    if (!address || !token) {
      resolve({ ok: false, reason: 'unavailable' })
      return
    }
    let text = ''
    let settled = false
    const finish = (value) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      socket.destroy()
      resolve(value)
    }
    const socket = connect(address)
    const timer = setTimeout(() => finish({ ok: false, reason: 'unavailable' }), timeoutMs)
    socket.setEncoding('utf8')
    socket.on('connect', () => socket.write(`${JSON.stringify({ token, prompt })}\n`))
    socket.on('data', (chunk) => (text += chunk))
    socket.on('error', () => finish({ ok: false, reason: 'unavailable' }))
    socket.on('close', () => {
      try {
        const reply = JSON.parse(text)
        if (reply?.ok === true && typeof reply.secret === 'string' && reply.secret)
          finish({ ok: true, secret: reply.secret })
        else finish({ ok: false, reason: String(reply?.reason || 'unavailable') })
      } catch {
        finish({ ok: false, reason: 'unavailable' })
      }
    })
  })
}

if (process.env[ASKPASS_FLAG] === '1') {
  if (!invokedAsAskpass()) {
    // 不是 askpass 调用：不去要密码，也不让通道和令牌继续传给这个程序的子进程。
    for (const name of ASKPASS_VARIABLES) delete process.env[name]
    delete process.env.NODE_OPTIONS
  } else {
    const reply = await requestAskpassSecret(
      process.env[ASKPASS_CHANNEL],
      process.env[ASKPASS_TOKEN],
      askpassPrompt(),
    )
    if (!reply.ok) {
      // 问的不是目标机、问的不是密码：留下标记，Pisper 据此说清原因。已经给过一次（used）或通道已关：
      // 不留标记——接下来 ssh 自己的“Permission denied”就是结论（密码不对）。
      if (reply.reason === 'other_host') writeSync(2, `${ASKPASS_OTHER_HOST}\n`)
      else if (reply.reason === 'second_factor') writeSync(2, `${ASKPASS_REFUSED}\n`)
      process.exit(1)
    }
    // writeSync：Windows 和 macOS 上管道写入是异步的，直接 exit 会丢掉还没写出的密码。
    writeSync(1, `${reply.secret}\n`)
    process.exit(0)
  }
}
