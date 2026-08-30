import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import test from 'node:test'
import { createStaticHandler } from '../http/static-handler.mjs'

function captureResponse() {
  let result
  return {
    response: {
      get headersSent() {
        return Boolean(result)
      },
      writeHead(status, headers) {
        result = { status, headers }
      },
      end(body) {
        result = { ...result, body: Buffer.from(body).toString('utf8') }
      },
    },
    read() {
      return result
    },
  }
}

test('缺失的前端 assets 不回退为 index.html', async () => {
  const root = await mkdtemp('/tmp/pisper-static-handler-')
  const dist = join(root, 'dist')
  try {
    await mkdir(dist, { recursive: true })
    await writeFile(join(dist, 'index.html'), '<!doctype html>')
    const serve = createStaticHandler(root)
    const capture = captureResponse()

    await serve({}, capture.response, new URL('http://127.0.0.1/assets/missing.js'))

    const result = capture.read()
    assert.equal(result.status, 404)
    assert.equal(result.headers['Content-Type'], 'application/json; charset=utf-8')
    assert.deepEqual(JSON.parse(result.body), { error: '静态资源不存在。' })
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})
