// SSH 密码登录时，密码怎么到 ssh 手里：助手（runtime/workers/ssh-askpass.mjs）被 ssh 调起后，带着一次性令牌
// 和 ssh 给它的提示语来这里要，这里核对过才给。密码不放进任何进程的环境变量、命令行或文件——环境里只有通道地址
// 和令牌，ssh 的子进程继承了也拿不到密码；令牌只换一次密码，而且提示语必须是目标机在要密码。
// 通道只为一次连接开：POSIX 上是只有自己能进的目录（0700）里的 unix socket；Windows 上是名字带随机串的命名管道
// （Node 设不了管道的访问控制，用的是系统默认：创建者、管理员和 SYSTEM 可读写，其他账户只读——只读的一方发不了
// 令牌，要不到密码）。密码只给一次，给过之后通道还开着：ssh 接着问别的（验证码）时要能告诉它“这个不支持”，
// 而不是让它看起来像密码错了。ssh 退出或超时后关闭。
import { randomBytes, timingSafeEqual } from 'node:crypto'
import { createServer } from 'node:net'
import { join } from 'node:path'
import { askpassDecision } from './ssh-askpass-prompt.mjs'

// 认证阶段不会比连接超时长多少；到点还没人来要就关掉。
const CHANNEL_LIFETIME_MS = 60_000
const REQUEST_TIMEOUT_MS = 5_000
const MAX_REQUEST_BYTES = 16 * 1024

export function askpassChannelAddress(directory, platform = process.platform) {
  return platform === 'win32'
    ? `\\\\.\\pipe\\pisper-askpass-${randomBytes(16).toString('hex')}`
    : join(directory, 's')
}

function sameToken(given, expected) {
  const left = Buffer.from(String(given ?? ''), 'utf8')
  const right = Buffer.from(expected, 'utf8')
  return left.length === right.length && timingSafeEqual(left, right)
}

/**
 * 为一次 ssh 连接开通道。secret 只留在这个闭包里，答过一次或关闭后就放掉。
 * @param {{
 *   directory: string,
 *   secret: string,
 *   expect: string[],
 *   bare?: boolean,
 *   platform?: string,
 *   lifetimeMs?: number,
 * }} options
 * @returns {Promise<{ address: string, token: string, close: () => void }>}
 */
export async function openAskpassChannel({
  directory,
  secret,
  expect,
  bare = false,
  platform = process.platform,
  lifetimeMs = CHANNEL_LIFETIME_MS,
}) {
  const address = askpassChannelAddress(directory, platform)
  const token = randomBytes(32).toString('hex')
  let password = String(secret)
  let closed = false
  const sockets = new Set()
  const server = createServer((socket) => {
    sockets.add(socket)
    socket.setEncoding('utf8')
    socket.setTimeout(REQUEST_TIMEOUT_MS, () => socket.destroy())
    socket.on('close', () => sockets.delete(socket))
    socket.on('error', () => {})
    let text = ''
    let handled = false
    const reply = (value) => {
      handled = true
      socket.end(`${JSON.stringify(value)}\n`)
    }
    socket.on('data', (chunk) => {
      if (handled) return
      text += chunk
      if (text.length > MAX_REQUEST_BYTES) {
        socket.destroy()
        return
      }
      const end = text.indexOf('\n')
      if (end < 0) return
      let request = null
      try {
        request = JSON.parse(text.slice(0, end))
      } catch {}
      // 令牌不对：不说原因，也不占用这一次回答的机会。
      if (!request || typeof request !== 'object' || !sameToken(request.token, token)) {
        reply({ ok: false, reason: 'denied' })
        return
      }
      // 先看问的是什么：不是目标机在问、或问的不是密码（验证码等），即使密码已经给过也要如实说，
      // 这样“密码之后还要验证码”的机器报的是“不支持第二重验证”，不是“密码不对”。
      const decision = askpassDecision(request.prompt, { expect, bare })
      if (decision !== 'answer') {
        reply({ ok: false, reason: decision })
        return
      }
      // 密码错了 ssh 会换一种方式再问：同一次连接不送第二遍。
      if (!password) {
        reply({ ok: false, reason: 'used' })
        return
      }
      const answer = password
      password = ''
      reply({ ok: true, secret: answer })
    })
  })
  server.on('error', () => {})
  const close = () => {
    if (closed) return
    closed = true
    password = ''
    clearTimeout(timer)
    // unix socket 文件由 net 在关闭时删掉；所在的目录由调用方整个删除。
    server.close()
    for (const socket of sockets) socket.destroy()
  }
  const timer = setTimeout(close, lifetimeMs)
  timer.unref?.()
  await new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(address, () => {
      server.off('error', reject)
      resolve()
    })
  })
  return { address, token, close }
}
