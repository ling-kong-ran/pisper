// 会话列表“最近一条”：从会话文件尾部取最后一条有文字的用户 / 助手消息，跳过注入内容，后台读取并缓存。
import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  createLastMessageCache,
  LAST_MESSAGE_MAX_CHARS,
  lastMessageFromMessages,
  readLastMessage,
} from '../runtime/session-last-message.mjs'
import { ATTACHMENT_MARKER } from '../runtime/stream-projection.mjs'

const line = (message) => JSON.stringify({ type: 'message', id: 'x', message })

test('the last message skips tool results and injected attachment context', () => {
  const preview = lastMessageFromMessages([
    { role: 'user', content: 'first' },
    { role: 'assistant', content: [{ type: 'text', text: '## Done\n\n- **pushed** `abc`' }] },
    { role: 'toolResult', content: [{ type: 'text', text: 'tool output' }] },
    { role: 'assistant', content: [{ type: 'toolCall', name: 'bash' }] },
  ])
  assert.deepEqual(preview, { role: 'assistant', text: 'Done pushed abc' })
  assert.deepEqual(
    lastMessageFromMessages([{ role: 'user', content: `fix it${ATTACHMENT_MARKER}secret file` }]),
    { role: 'user', text: 'fix it' },
  )
  const long = lastMessageFromMessages([{ role: 'assistant', content: 'x'.repeat(500) }])
  assert.equal(long.text.length, LAST_MESSAGE_MAX_CHARS)
  assert.ok(long.text.endsWith('…'))
  assert.equal(lastMessageFromMessages([]), null)
})

test('session files are read from the tail, past a large final line', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-last-message-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const file = join(directory, 'session.jsonl')
  await writeFile(
    file,
    [
      JSON.stringify({ type: 'session', id: 's' }),
      line({ role: 'user', content: 'hello' }),
      line({ role: 'assistant', content: [{ type: 'text', text: 'reply' }] }),
      line({ role: 'toolResult', content: [{ type: 'text', text: 'y'.repeat(200_000) }] }),
      '',
    ].join('\n'),
  )
  assert.deepEqual(await readLastMessage(file), { role: 'assistant', text: 'reply' })
})

test('the cache answers right away, reads once per file version and survives read errors', async () => {
  const reads = []
  let release
  const gate = new Promise((resolve) => (release = resolve))
  const cache = createLastMessageCache({
    read: async (path) => {
      reads.push(path)
      await gate
      if (path === 'bad') throw new Error('gone')
      return { role: 'assistant', text: path }
    },
  })
  const info = { path: 'a', modified: new Date(1), messageCount: 2 }
  assert.equal(cache.get(info), null)
  assert.equal(cache.get(info), null)
  assert.equal(cache.get({ path: 'bad', modified: new Date(1) }), null)
  release()
  await new Promise((resolve) => setTimeout(resolve, 10))
  assert.deepEqual(reads, ['a', 'bad'])
  assert.deepEqual(cache.get(info), { role: 'assistant', text: 'a' })
  assert.equal(cache.get({ path: 'bad', modified: new Date(1) }), null)
  assert.deepEqual(reads, ['a', 'bad'])
  // 文件更新后先返回上一版，同时排队重读。
  assert.deepEqual(cache.get({ ...info, messageCount: 3 }), { role: 'assistant', text: 'a' })
  await new Promise((resolve) => setTimeout(resolve, 10))
  assert.deepEqual(reads, ['a', 'bad', 'a'])
})

test('the session list adds lastMessage from live messages or the background file cache', async () => {
  const { readFile } = await import('node:fs/promises')
  const lifecycle = await readFile('runtime/runtime/session-lifecycle.mjs', 'utf8')
  assert.match(
    lifecycle,
    /\.\.\.lastMessageField\(\s*active\s*\? lastMessageFromMessages\(active\.session\.messages\)\s*: this\.lastMessages\.get\(session\),?\s*\)/,
  )
  assert.match(lifecycle, /this\.lastMessages = createLastMessageCache\(\)/)
})
