import assert from 'node:assert/strict'
import test from 'node:test'
import { createApiHandler, requiredRuntimeFeature } from '../http/api-handler.mjs'
import { exportFileName, sessionToMarkdown } from '../services/session-export.mjs'

const MESSAGES = [
  { id: 'u1', role: 'user', text: 'Fix the failing CI', timestamp: '2026-10-30T10:00:00.000Z' },
  {
    id: 'a1',
    role: 'agent',
    text: 'Fixed it. See `auth.ts`.\n\n```ts\nconst a = 1\n```',
    timestamp: '2026-10-30T10:01:00.000Z',
    runActivity: {
      thinkingText: 'secret reasoning that must not be exported',
      tools: [
        { name: 'bash', args: { command: 'npm test -- `auth`' }, status: 'error' },
        { name: 'edit', args: { path: 'src/auth.ts' }, status: 'done' },
      ],
    },
    attachments: [{ kind: 'image', data: 'AAAA' }],
  },
  { id: 'x', role: 'system', text: 'ignored' },
  { id: 'a2', role: 'agent', text: '', error: 'Provider\nfailed', timestamp: null },
]

test('sessionToMarkdown writes turns, tool summaries and placeholders but no thinking or paths', () => {
  const markdown = sessionToMarkdown({
    session: { name: 'Fix CI', cwd: '/home/someone/code/pisper', model: 'anthropic/claude' },
    messages: MESSAGES,
    exportedAt: new Date('2026-10-30T11:00:00.000Z'),
    locale: 'en-US',
  })
  assert.match(markdown, /^# Fix CI\n/)
  assert.match(markdown, /- Project: pisper\n/)
  assert.doesNotMatch(markdown, /\/home\/someone/)
  assert.match(markdown, /- Model: anthropic\/claude/)
  assert.match(markdown, /## You · /)
  assert.match(markdown, /Fix the failing CI/)
  assert.match(markdown, /\*\*2 actions\*\*/)
  assert.match(markdown, /- bash `` npm test -- `auth` `` ✕/)
  assert.match(markdown, /- edit `src\/auth.ts`/)
  assert.match(markdown, /```ts\nconst a = 1\n```/)
  assert.match(markdown, /\[Image attachment 1\]/)
  assert.doesNotMatch(markdown, /AAAA/)
  assert.doesNotMatch(markdown, /secret reasoning/)
  assert.doesNotMatch(markdown, /ignored/)
  assert.match(markdown, /> Error: Provider failed/)
  assert.ok(markdown.endsWith('\n'))
  assert.doesNotMatch(markdown, /\n{3,}/)
  const zh = sessionToMarkdown({ session: { name: '' }, messages: [] })
  assert.match(zh, /^# Pisper\n/)
  assert.match(zh, /这个会话还没有消息/)
})

test('exportFileName strips path separators and control characters', () => {
  assert.equal(exportFileName('Fix: CI / auth?'), 'Fix CI auth.md')
  assert.equal(exportFileName('修复 登录'), '修复 登录.md')
  assert.equal(exportFileName('../..'), 'pisper-session.md')
  assert.equal(exportFileName(''), 'pisper-session.md')
  assert.equal(exportFileName('a\u0007b'), 'a b.md')
})

test('export route returns a markdown attachment, 404 for unknown sessions, and is gated', async () => {
  assert.equal(requiredRuntimeFeature('/api/sessions/s/export'), 'sessionExport')
  const calls = []
  const runtime = {
    capabilities: { features: { sessionExport: true } },
    async exportSessionMarkdown(id, options) {
      calls.push([id, options.locale])
      return id === 's1' ? { fileName: '修复.md', markdown: '# 修复\n' } : null
    },
  }
  const handler = createApiHandler(runtime)
  const get = async (path) => {
    const response = {
      status: 0,
      headers: {},
      body: null,
      writeHead(status, headers = {}) {
        this.status = status
        Object.assign(this.headers, headers)
      },
      setHeader(name, value) {
        this.headers[name] = value
      },
      end(body) {
        this.body = body
      },
    }
    await handler(
      { method: 'GET', headers: {}, async *[Symbol.asyncIterator]() {} },
      response,
      new URL(`http://localhost${path}`),
    )
    return response
  }
  const ok = await get('/api/sessions/s1/export?format=markdown&lang=en-US')
  assert.equal(ok.status, 200)
  assert.equal(ok.headers['Content-Type'], 'text/markdown; charset=utf-8')
  assert.match(ok.headers['Content-Disposition'], /filename\*=UTF-8''%E4%BF%AE%E5%A4%8D\.md/)
  assert.equal(Buffer.from(ok.body).toString('utf8'), '# 修复\n')
  assert.deepEqual(calls, [['s1', 'en-US']])
  assert.equal((await get('/api/sessions/missing/export')).status, 404)
  assert.equal((await get('/api/sessions/s1/export?format=pdf')).status, 400)
  const gated = createApiHandler({ capabilities: { features: { sessionExport: false } } })
  const response = {
    status: 0,
    body: '',
    writeHead(status) {
      this.status = status
    },
    setHeader() {},
    end(body) {
      this.body = body
    },
  }
  await gated(
    { method: 'GET', headers: {}, async *[Symbol.asyncIterator]() {} },
    response,
    new URL('http://localhost/api/sessions/s1/export'),
  )
  assert.equal(response.status, 409)
})
