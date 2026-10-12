import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { EventEmitter, once } from 'node:events'
import fs from 'node:fs'
import http from 'node:http'
import net from 'node:net'
import { createInterface } from 'node:readline'
import test from 'node:test'
import tls from 'node:tls'
import { zstdDecompressSync } from 'node:zlib'

const fixtureUrl = new URL(
  '../../runtime-rs/src/native_channels/feishu_qq/sdk-proxy-oracle.mjs',
  import.meta.url,
)
const ca = fs.readFileSync(new URL('proxy-test-ca.pem', fixtureUrl))

// 观测和故障注入只存在于子进程测试引导中，不改变夹具协议或路由记录。
async function observeFixture(url) {
  const { default: net } = await import('node:net')
  const { default: http } = await import('node:http')
  const { errorMonitor } = await import('node:events')
  const connections = new Map()
  let current,
    sequence = 0
  const send = (message) => process.send(message)
  const connect = net.connect
  net.connect = function (...args) {
    const socket = connect.apply(this, args)
    const connection = current
    if (connection) {
      connection.upstream = socket
      socket.once('close', () => send({ type: 'upstream-close', id: connection.id }))
    }
    return socket
  }
  const emit = http.Server.prototype.emit
  http.Server.prototype.emit = function (event, ...args) {
    if (event !== 'connect') return emit.call(this, event, ...args)
    const [request, client] = args
    const connection = { id: ++sequence, client, target: request.url }
    connections.set(connection.id, connection)
    // errorMonitor 不会处理 error；原夹具仍会因缺少真正的 handler 而退出。
    client.on(errorMonitor, (error) =>
      send({ type: 'client-error', id: connection.id, code: error.code }),
    )
    client.once('close', () => send({ type: 'client-close', id: connection.id }))
    current = connection
    try {
      return emit.call(this, event, ...args)
    } finally {
      current = undefined
      send({
        type: 'connect',
        id: connection.id,
        target: connection.target,
        clientPort: client.remotePort,
      })
    }
  }
  process.on('message', ({ type, id, side, code }) => {
    if (type !== 'inject-error') return
    const socket = connections.get(id)[side]
    socket.destroy(Object.assign(new Error('synthetic transport failure'), { code }))
  })
  process.channel.unref()
  process.argv[2] = '--serve'
  await import(url)
}

async function startFixture(t) {
  const child = spawn(
    process.env.PISPER_TEST_NODE24 || process.execPath,
    [
      '--input-type=module',
      '--eval',
      `(${observeFixture.toString()})(${JSON.stringify(fixtureUrl.href)})`,
    ],
    {
      env: { PATH: process.env.PATH, SYSTEMROOT: process.env.SYSTEMROOT || 'C:/Windows' },
      stdio: ['pipe', 'pipe', 'pipe', 'ipc'],
      windowsHide: true,
    },
  )
  const updates = new EventEmitter()
  const messages = []
  const sockets = new Set()
  let stderr = '',
    endpoints,
    exit,
    spawnError
  child.stderr.on('data', (value) => {
    stderr += value
  })
  child.on('error', (error) => {
    spawnError = error
    updates.emit('update')
  })
  child.on('message', (message) => {
    messages.push(message)
    updates.emit('update')
  })
  const lines = createInterface({ input: child.stdout })
  lines.on('line', (line) => {
    endpoints = JSON.parse(line).endpoints
    updates.emit('update')
  })
  const closed = new Promise((resolve) =>
    child.once('close', (code, signal) => {
      exit = { code, signal }
      updates.emit('update')
      resolve(exit)
    }),
  )
  t.after(async () => {
    for (const socket of sockets) socket.destroy()
    if (!exit) child.kill()
    await closed
    lines.close()
  })
  const waitFor = (read, description) =>
    new Promise((resolve, reject) => {
      const cleanup = () => {
        updates.off('update', check)
        t.signal.removeEventListener('abort', abort)
      }
      const abort = () => {
        cleanup()
        reject(t.signal.reason)
      }
      const check = () => {
        const value = read()
        if (value) {
          cleanup()
          resolve(value)
        } else if (exit || spawnError) {
          cleanup()
          reject(
            new assert.AssertionError({
              message: `fixture exited before ${description}: ${JSON.stringify(exit)}\n${stderr}`,
              actual: spawnError || exit,
              expected: 'running',
            }),
          )
        }
      }
      updates.on('update', check)
      t.signal.addEventListener('abort', abort, { once: true })
      check()
    })
  await waitFor(() => endpoints, 'startup')
  return {
    child,
    endpoints,
    sockets,
    get stderr() {
      return stderr
    },
    message: (type, id) =>
      waitFor(
        () =>
          messages.find((value) => value.type === type && (id === undefined || value.id === id)),
        type,
      ),
    connection: (clientPort) =>
      waitFor(
        () => messages.find((value) => value.type === 'connect' && value.clientPort === clientPort),
        'CONNECT association',
      ),
    exit: () => waitFor(() => exit, 'expected failure'),
    stop() {
      child.stdin.end('stop\n')
      return waitFor(() => exit, 'shutdown')
    },
  }
}

async function openTunnel(fixture, target) {
  const proxy = new URL(fixture.endpoints.HTTP_PROXY)
  const socket = net.connect({ host: proxy.hostname, port: Number(proxy.port) })
  fixture.sockets.add(socket)
  // 故障用例会主动关闭连接，客户端仍由正文与进程状态断言验证结果。
  socket.on('error', () => {})
  await once(socket, 'connect')
  const headers = new Promise((resolve, reject) => {
    let bytes = Buffer.alloc(0)
    const cleanup = () => {
      socket.off('data', data)
      socket.off('error', error)
      socket.off('end', end)
    }
    const error = (error) => {
      cleanup()
      reject(error)
    }
    const end = () => error(new Error('CONNECT ended before response headers'))
    const data = (chunk) => {
      bytes = Buffer.concat([bytes, chunk])
      if (bytes.includes('\r\n\r\n')) {
        socket.pause()
        cleanup()
        resolve(bytes.toString())
      }
    }
    socket.on('data', data)
    socket.once('error', error)
    socket.once('end', end)
  })
  socket.write(`CONNECT ${target.host} HTTP/1.1\r\nHost: ${target.host}\r\n\r\n`)
  assert.match(await headers, /^HTTP\/1\.1 200 /)
  const connection = await fixture.connection(socket.localPort)
  return { socket, id: connection.id }
}

function get(url) {
  return new Promise((resolve, reject) => {
    const request = http.get(url, { agent: false }, (response) => {
      const chunks = []
      response.on('data', (chunk) => chunks.push(chunk))
      response.on('error', reject)
      response.on('end', () =>
        resolve({ status: response.statusCode, body: Buffer.concat(chunks).toString() }),
      )
    })
    request.on('error', reject)
  })
}

async function nestedZstd(fixture) {
  const { socket } = await openTunnel(fixture, new URL(fixture.endpoints.HTTPS_PROXY))
  const secure = tls.connect({ socket, ca })
  fixture.sockets.add(secure)
  await once(secure, 'secureConnect')
  const target = new URL(fixture.endpoints.HTTP_ORIGIN + '/body/zstd')
  secure.write(`GET ${target.href} HTTP/1.1\r\nHost: ${target.host}\r\nConnection: close\r\n\r\n`)
  const chunks = []
  for await (const chunk of secure) chunks.push(chunk)
  const wire = Buffer.concat(chunks)
  const split = wire.indexOf('\r\n\r\n')
  assert.ok(split > 0)
  const headers = wire.subarray(0, split).toString()
  assert.match(headers, /^HTTP\/1\.1 200 /)
  assert.match(headers, /content-encoding: zstd/i)
  const length = Number(/content-length: (\d+)/i.exec(headers)?.[1])
  const body = wire.subarray(split + 4)
  assert.equal(body.length, length)
  return zstdDecompressSync(body).toString()
}

test(
  'CONNECT client reset closes its upstream and preserves healthy nested zstd requests',
  { timeout: 10000 },
  async (t) => {
    const fixture = await startFixture(t)
    const { socket, id } = await openTunnel(fixture, new URL(fixture.endpoints.HTTPS_ORIGIN))
    socket.resetAndDestroy()
    assert.equal((await fixture.message('client-error', id)).code, 'ECONNRESET')
    await fixture.message('upstream-close', id)
    await fixture.message('client-close', id)
    await get(fixture.endpoints.HTTP_ORIGIN + '/__records')
    assert.deepEqual(await get(fixture.endpoints.HTTP_ORIGIN + '/final'), {
      status: 200,
      body: 'synthetic proxied media',
    })
    assert.equal(await nestedZstd(fixture), 'synthetic proxied media')
    const records = JSON.parse((await get(fixture.endpoints.HTTP_ORIGIN + '/__records')).body)
    assert.deepEqual(
      records.map(({ server, method }) => `${server}:${method}`),
      ['HTTP_ORIGIN:GET', 'HTTP_PROXY:CONNECT', 'HTTPS_PROXY:GET', 'HTTP_ORIGIN:GET'],
    )
    assert.deepEqual(await fixture.stop(), { code: 0, signal: null })
  },
)

for (const [side, code] of [
  ['client', 'EIO'],
  ['upstream', 'EIO'],
  ['upstream', 'ECONNRESET'],
]) {
  test(
    `CONNECT ${side} ${code} remains a contextual fixture failure`,
    { timeout: 10000 },
    async (t) => {
      const fixture = await startFixture(t)
      const target = new URL(fixture.endpoints.HTTPS_ORIGIN)
      const { id } = await openTunnel(fixture, target)
      fixture.child.send({ type: 'inject-error', id, side, code })
      assert.deepEqual(await fixture.exit(), { code: 1, signal: null })
      assert.ok(fixture.stderr.includes('HTTP_PROXY'), fixture.stderr)
      assert.ok(fixture.stderr.includes(target.host), fixture.stderr)
      assert.ok(fixture.stderr.includes(side), fixture.stderr)
      assert.ok(fixture.stderr.includes(code), fixture.stderr)
      assert.ok(fixture.stderr.includes('synthetic transport failure'), fixture.stderr)
    },
  )
}
