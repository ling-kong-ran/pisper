import assert from 'node:assert/strict'
import { createCipheriv, createHash } from 'node:crypto'
import { readFile, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { dirname, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

// 只导入只读 release 源码；协议请求由本进程创建的 loopback HTTP 夹具处理。
const here = dirname(fileURLToPath(import.meta.url))
const reference = resolve(here, '../../../../pisper-release-parity-reference')
const source = (name) => resolve(reference, 'runtime/services/channels', name)
const { TelegramGateway } = await import(pathToFileURL(source('telegram-gateway.mjs')))
const { WeixinGateway } = await import(pathToFileURL(source('weixin-gateway.mjs')))
const { WeixinProtocol } = await import(pathToFileURL(source('weixin-protocol.mjs')))
const telegramRaw = {
  message_id: 12,
  chat: { id: -100, type: 'supergroup' },
  from: { id: 7, first_name: 'Ada' },
  caption: '请看图',
  photo: [{ file_id: 'small' }, { file_id: 'large' }],
}
const weixinRaw = {
  message_id: 9,
  from_user_id: 'wx-owner',
  context_token: 'reply-context',
  item_list: [
    { type: 1, text_item: { text: ' hello ' } },
    { type: 3, voice_item: { text: 'voice' } },
    { type: 4, file_item: { file_name: 'actual.txt', media: { encrypt_query_param: 'owned' } } },
  ],
}
let updates = 0
const headers = []
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, 'http://127.0.0.1')
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    const body = JSON.parse(Buffer.concat(chunks).toString() || '{}')
    headers.push({ path: url.pathname, headers: request.headers, body })
    let value = { ret: 0 }
    if (url.pathname.endsWith('/getupdates')) {
      if (updates++ > 0) return
      value = { ret: 0, msgs: [weixinRaw], get_updates_buf: 'next-cursor' }
    } else if (url.pathname.endsWith('/sendmessage')) {
      value = { ret: -2, errmsg: 'prepare failed' }
    }
    response.writeHead(200, { 'Content-Type': 'application/json' })
    response.end(JSON.stringify(value))
  } catch (error) {
    response.writeHead(500, { 'Content-Type': 'application/json' })
    response.end(JSON.stringify({ error: error.message }))
  }
})
await new Promise((done) => server.listen(0, '127.0.0.1', done))
const base = `http://127.0.0.1:${server.address().port}`
let gateway
try {
  const telegram = new TelegramGateway()
  const protocol = new WeixinProtocol()
  let captured
  let received
  const message = new Promise((done) => {
    received = done
  })
  gateway = new WeixinGateway({
    protocol,
    onMessage: (value) => {
      captured = value
      received()
    },
  })
  await gateway.connect({ token: 'synthetic-node-oracle', baseUrl: base, syncBuf: '' })
  await message
  let expiredContextError
  try {
    await protocol.sendText(
      { token: 'synthetic-node-oracle', baseUrl: base },
      { to: 'wx-owner', text: 'message', contextToken: 'expired' },
    )
    assert.fail('Expired context must fail')
  } catch (error) {
    expiredContextError = error.message
  }
  const plain = Buffer.from('Pisper media bytes\0中文', 'utf8')
  const key = Buffer.from('000102030405060708090a0b0c0d0e0f', 'hex')
  const cipher = createCipheriv('aes-128-ecb', key, null)
  const encrypted = Buffer.concat([cipher.update(plain), cipher.final()])
  const vector = {
    releaseCommit: '5821602',
    sourceSha256: Object.fromEntries(
      await Promise.all(
        [
          'telegram-gateway.mjs',
          'weixin-gateway.mjs',
          'weixin-protocol.mjs',
          'weixin-onboarding.mjs',
        ].map(async (name) => [
          name,
          createHash('sha256')
            .update(await readFile(source(name)))
            .digest('hex'),
        ]),
      ),
    ),
    telegramRaw,
    telegramMapped: telegram.mapMessage(telegramRaw),
    weixinRaw,
    weixinMapped: captured,
    crypto: { plaintextHex: plain.toString('hex'), ciphertextHex: encrypted.toString('hex') },
    expiredContextError,
    protocol: {
      clientVersion: headers[0].headers['ilink-app-clientversion'],
      baseInfo: headers[0].body.base_info,
    },
  }
  assert.equal(vector.protocol.clientVersion, '132102')
  await writeFile(
    resolve(here, 'telegram_weixin_oracle.json'),
    JSON.stringify(vector, null, 2) + '\n',
  )
  console.log('Read-only release Telegram/Weixin oracle captured through owned loopback HTTP')
} finally {
  await gateway?.disconnect()
  server.closeAllConnections()
  await new Promise((done) => server.close(done))
}
