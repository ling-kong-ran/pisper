// 真机验证用的最小 CDP 客户端：对 Android WebView 页面执行一段 JS 并打印结果。
// 用法：node scripts/dev-cdp-eval.mjs <cdpPort> <pageId> <表达式文件>
// 仅用于本地开发验证，不参与构建与发布。
import { readFileSync } from 'node:fs'
import http from 'node:http'
import crypto from 'node:crypto'

const [port, pageId, expressionFile] = process.argv.slice(2)
if (!port || !pageId || !expressionFile) {
  console.error('用法: node dev-cdp-eval.mjs <port> <pageId> <expressionFile>')
  process.exit(1)
}
const expression = readFileSync(expressionFile, 'utf8')

// 手写最小 WebSocket 客户端（避免为验证脚本引入 ws 依赖）。
function wsConnect(path) {
  return new Promise((resolve, reject) => {
    const key = crypto.randomBytes(16).toString('base64')
    const request = http.request({
      host: '127.0.0.1',
      port: Number(port),
      path,
      headers: {
        Connection: 'Upgrade',
        Upgrade: 'websocket',
        'Sec-WebSocket-Key': key,
        'Sec-WebSocket-Version': '13',
      },
    })
    request.on('upgrade', (response, socket) => resolve(socket))
    request.on('error', reject)
    request.end()
  })
}

function encodeFrame(text) {
  const payload = Buffer.from(text)
  const mask = crypto.randomBytes(4)
  const header = []
  header.push(0x81)
  if (payload.length < 126) {
    header.push(0x80 | payload.length)
  } else if (payload.length < 65536) {
    header.push(0x80 | 126, (payload.length >> 8) & 0xff, payload.length & 0xff)
  } else {
    header.push(0x80 | 127, 0, 0, 0, 0)
    const big = Buffer.alloc(4)
    big.writeUInt32BE(payload.length)
    header.push(...big)
  }
  const masked = Buffer.from(payload)
  for (let index = 0; index < masked.length; index += 1) masked[index] ^= mask[index % 4]
  return Buffer.concat([Buffer.from(header), mask, masked])
}

function decodeFrames(buffer, onMessage) {
  let offset = 0
  const messages = []
  while (offset + 2 <= buffer.length) {
    const lengthByte = buffer[offset + 1] & 0x7f
    let headerSize = 2
    let payloadLength = lengthByte
    if (lengthByte === 126) {
      payloadLength = buffer.readUInt16BE(offset + 2)
      headerSize = 4
    } else if (lengthByte === 127) {
      payloadLength = Number(buffer.readBigUInt64BE(offset + 2))
      headerSize = 10
    }
    if (offset + headerSize + payloadLength > buffer.length) break
    const payload = buffer.subarray(offset + headerSize, offset + headerSize + payloadLength)
    messages.push(payload.toString('utf8'))
    offset += headerSize + payloadLength
  }
  for (const message of messages) onMessage(message)
  return buffer.subarray(offset)
}

const socket = await wsConnect(`/devtools/page/${pageId}`)
let pending = Buffer.alloc(0)
let messageId = 0
const waiters = new Map()

socket.on('data', (chunk) => {
  pending = decodeFrames(Buffer.concat([pending, chunk]), (message) => {
    let parsed
    try {
      parsed = JSON.parse(message)
    } catch {
      return
    }
    if (parsed.id && waiters.has(parsed.id)) {
      waiters.get(parsed.id)(parsed)
      waiters.delete(parsed.id)
    }
  })
})

function send(method, params) {
  messageId += 1
  const id = messageId
  socket.write(encodeFrame(JSON.stringify({ id, method, params })))
  return new Promise((resolve) => waiters.set(id, resolve))
}

const result = await send('Runtime.evaluate', {
  expression,
  awaitPromise: true,
  returnByValue: true,
})
console.log(JSON.stringify(result.result?.result?.value ?? result.result, null, 2))
socket.end()
process.exit(0)
