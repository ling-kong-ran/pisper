import assert from 'node:assert/strict'
import test from 'node:test'
import { collectTaskSources } from '../../src/features/chat/task-sources.ts'

test('source identity uses asset IDs and paths, never display names', () => {
  const sources = collectTaskSources([
    {
      id: 'm',
      role: 'user',
      attachments: [
        { id: 'one', name: 'report.md', mimeType: 'text/markdown' },
        { id: 'two', name: 'report.md', mimeType: 'text/markdown' },
        { id: 'one', name: 'renamed.md' },
        { id: 'path:/project/a.txt', path: '/project/a.txt', name: 'a.txt' },
        { path: '/project/./a.txt', name: 'a.txt' },
      ],
    },
  ])
  assert.equal(sources.length, 3)
  assert.deepEqual(
    sources.map((s) => s.id),
    ['asset:one', 'asset:two', 'path:/project/a.txt'],
  )
  assert.equal(sources[0].kind, 'text')
  assert.equal(sources[0].attachment.downloadUrl, '/api/assets/one/download')
})

test('malformed, unresolved and hostile references remain safe and distinguishable', () => {
  const sources = collectTaskSources([
    {
      id: 'm',
      role: 'user',
      attachments: [
        null,
        42,
        { name: 'lost.pdf', url: 'javascript:alert(1)' },
        {
          id: 'safe',
          name: 'picture.png',
          mimeType: 'image/png',
          url: 'https://host/secret',
          downloadUrl: 'javascript:bad',
          data: 'bad',
        },
        { path: 'javascript:alert(1)', name: 'bad' },
      ],
    },
    { id: 'b', role: 'assistant', attachments: 'invalid' },
  ])
  assert.equal(sources.length, 3)
  assert.equal(sources[0].available, false)
  assert.equal(sources[1].available, true)
  assert.equal(sources[1].attachment.url, undefined)
  assert.equal(sources[1].attachment.data, undefined)
  assert.equal(sources[2].available, false)
  assert.deepEqual(collectTaskSources([]), [])
})
