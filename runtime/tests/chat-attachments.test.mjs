import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  CHAT_ATTACHMENT_ACCEPT,
  clipboardFiles,
  pathAttachments,
} from '../../src/features/chat/attachments.ts'

test('clipboard files include images and general files', () => {
  const image = { name: 'screenshot.png', type: 'image/png' }
  const document = { name: 'notes.pdf', type: 'application/pdf' }

  assert.deepEqual(clipboardFiles({ files: [image, document] }), [image, document])
})

test('clipboard file items are supported when the files collection is empty', () => {
  const image = { name: 'clipboard.png', type: 'image/png' }
  const document = { name: 'notes.md', type: 'text/markdown' }
  const items = [
    { kind: 'string', type: 'text/plain', getAsFile: () => null },
    { kind: 'file', type: 'image/png', getAsFile: () => image },
    { kind: 'file', type: 'text/markdown', getAsFile: () => document },
  ]

  assert.deepEqual(clipboardFiles({ files: [], items }), [image, document])
})

test('plain text clipboard data does not trigger attachment handling', () => {
  assert.deepEqual(
    clipboardFiles({ files: [], items: [{ kind: 'string', type: 'text/plain' }] }),
    [],
  )
})

test('system picker advertises every supported mobile attachment family', () => {
  for (const type of ['image/*', 'text/*', 'application/json', '.pdf', '.docx', '.md']) {
    assert.ok(CHAT_ATTACHMENT_ACCEPT.split(',').includes(type))
  }
})

test('mobile chat uses the system file picker while desktop keeps path attachments', async () => {
  const [focus, picker] = await Promise.all([
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/AttachmentPicker.tsx', 'utf8'),
  ])
  assert.match(focus, /<AttachmentPicker cwd=\{cwd\} selection=\{selection\} \/>/)
  assert.match(picker, /const mobileApp = useIsMobileApp\(\)/)
  assert.match(picker, /if \(mobileApp\) \{[\s\S]*deviceFileInputRef\.current\?\.click\(\)/)
  assert.match(picker, /type="file"[\s\S]*accept=\{CHAT_ATTACHMENT_ACCEPT\}[\s\S]*multiple/)
  assert.match(picker, /onChange=\{addDeviceFiles\}/)
  assert.match(picker, /selection\.addFiles\(files\)/)
  assert.match(picker, /window\.pisperDesktop\?\.pickFiles/)
  assert.match(picker, /selection\.addAttachments\(pathAttachments\(paths \|\| \[\]\)\)/)
})

test('selected files become path-only attachments without content or size fields', () => {
  assert.deepEqual(pathAttachments(['E:\\workspace\\large.bin', '/workspace/notes.md']), [
    {
      id: 'path:E:\\workspace\\large.bin',
      kind: 'path',
      name: 'large.bin',
      path: 'E:\\workspace\\large.bin',
    },
    {
      id: 'path:/workspace/notes.md',
      kind: 'path',
      name: 'notes.md',
      path: '/workspace/notes.md',
    },
  ])
})
