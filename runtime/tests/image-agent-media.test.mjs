import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtemp, mkdir, open, rm, symlink, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { PNG } from 'pngjs'
import { createServer } from 'node:net'
import { importAgentImage } from '../services/image-agent-media.mjs'
import { ImageToolsPlugin } from '../services/image-tools-plugin.mjs'
import { WorkflowMediaService } from '../services/workflow-media-service.mjs'

const png = () => PNG.sync.write({ width: 2, height: 2, data: Buffer.alloc(16, 200) })
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-image-agent-'))
  const cwd = join(root, 'workspace'),
    media = new WorkflowMediaService({ dataDir: join(root, 'managed') })
  await mkdir(cwd)
  t.after(async () => {
    await media.dispose()
    await rm(root, { recursive: true, force: true })
  })
  let enabled = true,
    calls = 0
  const plugin = new ImageToolsPlugin({
    isAgentEnabled: () => {
      calls++
      return enabled
    },
  })
  return {
    root,
    cwd,
    media,
    plugin,
    disable: () => {
      enabled = false
    },
    count: () => calls,
  }
}
const safeCode = (code, privatePath) => (cause) => {
  assert.equal(cause.code, code)
  assert.equal(cause.message.includes(privatePath), false)
  return true
}

test('agent imports verified workspace images into its injected media store and keeps path private', async (t) => {
  const context = await fixture(t)
  const buffer = png()
  await mkdir(join(context.cwd, 'sprites'))
  await writeFile(join(context.cwd, 'sprites', 'character.data'), buffer)
  const result = await importAgentImage({ ...context, sourceImage: 'sprites/character.data' })
  assert.equal(result.name, 'character.data')
  assert.equal(result.mimeType, 'image/png')
  assert.equal(result.size, buffer.length)
  assert.equal(JSON.stringify(result).includes(context.cwd), false)
  assert.equal(context.count(), 2)
  const stored = await context.media.read(result.id)
  assert.deepEqual(stored.buffer, buffer)
  context.disable()
  await assert.rejects(importAgentImage({ ...context, sourceImage: 'sprites/character.data' }), {
    code: 'image_tools_agent_disabled',
  })
})

test('agent import rejects paths outside the canonical workspace, URLs, symlinks and directories', async (t) => {
  const context = await fixture(t)
  await writeFile(join(context.root, 'private.png'), png())
  await writeFile(join(context.cwd, 'safe.png'), png())
  await mkdir(join(context.cwd, 'nested'))
  await symlink(join(context.root, 'private.png'), join(context.cwd, 'outside.png'))
  await symlink(join(context.cwd, 'safe.png'), join(context.cwd, 'inside.png'))
  await symlink(
    context.root,
    join(context.cwd, 'redirect'),
    process.platform === 'win32' ? 'junction' : 'dir',
  )
  for (const sourceImage of ['../private.png', join(context.root, 'private.png')])
    await assert.rejects(
      importAgentImage({ ...context, sourceImage }),
      safeCode('image_tools_source_outside_workspace', context.root),
    )
  for (const sourceImage of [
    '',
    'https://example.test/image.png',
    'file:///private.png',
    'data:image/png;base64,AAA',
    'outside.png',
    'inside.png',
    'redirect/private.png',
    'nested',
    '\0secret',
  ])
    await assert.rejects(
      importAgentImage({ ...context, sourceImage }),
      safeCode('image_tools_source_invalid', context.root),
    )
  await assert.rejects(
    importAgentImage({ ...context, sourceImage: 'missing.png' }),
    safeCode('image_tools_source_unavailable', context.root),
  )
})

test('agent import uses true image metadata and rejects oversized bytes and decompression dimensions', async (t) => {
  const context = await fixture(t)
  await writeFile(join(context.cwd, 'fake.png'), 'not an image')
  await assert.rejects(importAgentImage({ ...context, sourceImage: 'fake.png' }), {
    code: 'image_tools_source_invalid',
  })
  const huge = await open(join(context.cwd, 'huge.png'), 'w')
  await huge.truncate(8 * 1024 * 1024 + 1)
  await huge.close()
  await assert.rejects(importAgentImage({ ...context, sourceImage: 'huge.png' }), {
    code: 'image_tools_source_too_large',
  })
  const oversized = png()
  oversized.writeUInt32BE(8000, 16)
  await writeFile(join(context.cwd, 'dimensions.png'), oversized)
  await assert.rejects(importAgentImage({ ...context, sourceImage: 'dimensions.png' }), {
    code: 'image_tools_source_too_large',
  })
})

test(
  'agent import rejects special files without waiting for stream data',
  { skip: process.platform === 'win32' },
  async (t) => {
    const context = await fixture(t)
    // macOS 对 Unix socket 路径长度限制较低，短路径避免测试依赖个人临时目录长度。
    const socketDir = await mkdtemp('/tmp/pisper-sock-')
    const server = createServer()
    const socket = join(socketDir, 'file')
    try {
      await new Promise((resolve, reject) => {
        server.once('error', reject)
        server.listen(socket, resolve)
      })
      await assert.rejects(importAgentImage({ ...context, cwd: socketDir, sourceImage: 'file' }), {
        code: 'image_tools_source_invalid',
      })
    } finally {
      await new Promise((resolve, reject) =>
        server.close((cause) => (cause ? reject(cause) : resolve())),
      )
      await rm(socketDir, { recursive: true, force: true })
    }
  },
)

test('agent import cancellation and mid-read disable prevent persistence', async (t) => {
  const context = await fixture(t)
  await writeFile(join(context.cwd, 'safe.png'), png())
  let uploads = 0,
    checks = 0
  const media = {
    upload: async () => {
      uploads++
      throw new Error('unexpected upload')
    },
  }
  const cancelled = new AbortController()
  cancelled.abort()
  await assert.rejects(
    importAgentImage({ ...context, media, sourceImage: 'safe.png', signal: cancelled.signal }),
    { name: 'AbortError' },
  )
  const controller = new AbortController()
  await assert.rejects(
    importAgentImage({
      ...context,
      media,
      sourceImage: 'safe.png',
      signal: controller.signal,
      plugin: {
        assertAllowed: async () => {
          checks++
          if (checks === 2) controller.abort()
        },
      },
    }),
    { name: 'AbortError' },
  )
  checks = 0
  const plugin = new ImageToolsPlugin({ isAgentEnabled: () => ++checks === 1 })
  await assert.rejects(importAgentImage({ ...context, plugin, media, sourceImage: 'safe.png' }), {
    code: 'image_tools_agent_disabled',
  })
  assert.equal(uploads, 0)
})

test('agent import sanitizes storage errors containing private paths', async (t) => {
  const context = await fixture(t)
  await writeFile(join(context.cwd, 'safe.png'), png())
  const media = {
    upload: async () => {
      throw new Error(`Failure writing ${context.root}/private-store`)
    },
  }
  await assert.rejects(
    importAgentImage({ ...context, media, sourceImage: 'safe.png' }),
    safeCode('image_tools_import_failed', context.root),
  )
})
