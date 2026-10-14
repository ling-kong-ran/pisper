import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'
import {
  mkdtemp,
  mkdir,
  readFile,
  readdir,
  realpath,
  rm,
  symlink,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'
import { PNG } from 'pngjs'
import { exportAgentImages } from '../services/image-agent-export.mjs'
import { ImageToolsPlugin } from '../services/image-tools-plugin.mjs'

async function fixture(t) {
  const temporary = await mkdtemp(join(tmpdir(), 'pisper-image-export-'))
  const root = await realpath(temporary)
  const cwd = join(root, 'workspace')
  await mkdir(cwd)
  t.after(() => rm(root, { recursive: true, force: true }))
  const image = new PNG({ width: 32, height: 16 })
  image.data.fill(255)
  const buffer = PNG.sync.write(image)
  const reference = {
    id: randomUUID(),
    name: 'private-atlas.png',
    mimeType: 'image/png',
    size: buffer.length,
  }
  const frame = {
    media: reference,
    width: 16,
    height: 16,
    durationMs: 125,
    action: 'walk',
    direction: 'S',
    columns: 1,
    rows: 1,
    frameCount: 1,
  }
  const frames = [0, 16].map((x) => ({
    x,
    y: 0,
    width: 16,
    height: 16,
    durationMs: 125,
    action: 'walk',
    direction: 'S',
  }))
  const output = {
    type: 'workflow-images',
    version: 1,
    frames: [frame, frame],
    atlas: { media: reference, width: 32, height: 16, frames },
  }
  const media = {
    read: (id) => {
      assert.equal(id, reference.id)
      return Promise.resolve({ media: reference, buffer })
    },
  }
  const plugin = new ImageToolsPlugin({ isAgentEnabled: () => true })
  return { root, cwd, output, reference, media, plugin, buffer }
}

test('Agent exports a verified atlas and portable frame JSON to a fresh workspace directory', async (t) => {
  const input = await fixture(t)
  const first = await exportAgentImages(input)
  const second = await exportAgentImages(input)
  assert.equal(first.files.length, 2)
  assert.deepEqual(
    first.files.map((file) => file.mimeType),
    ['image/png', 'application/json'],
  )
  assert.notEqual(dirname(first.files[0].path), dirname(second.files[0].path))
  assert.match(
    first.files[0].path.slice(input.cwd.length),
    /^[/\\]generated[/\\]image-assets[/\\][0-9a-f-]{36}[/\\]atlas\.png$/,
  )
  assert.deepEqual(await readFile(first.files[0].path), input.buffer)
  const text = await readFile(first.files[1].path, 'utf8')
  const metadata = JSON.parse(text)
  assert.equal(metadata.image, 'atlas.png')
  assert.equal(metadata.width, 32)
  assert.equal(metadata.height, 16)
  assert.deepEqual(metadata.frames, input.output.atlas.frames)
  assert.doesNotMatch(text, /private-atlas|mimeType|"media"|"id"/)
  assert.ok(!text.includes(input.reference.id))
  assert.equal((await readdir(join(input.cwd, 'generated', 'image-assets'))).length, 2)
})

test('disabled Agent capability rejects export before reading media or writing directories', async (t) => {
  const input = await fixture(t)
  let read = false
  await assert.rejects(
    exportAgentImages({
      ...input,
      plugin: new ImageToolsPlugin(),
      media: {
        read: () => {
          read = true
          return input.media.read(input.reference.id)
        },
      },
    }),
    { code: 'image_tools_agent_disabled', statusCode: 403 },
  )
  assert.equal(read, false)
  assert.deepEqual(await readdir(input.cwd), [])
})

test('foreign, forged, oversized and inconsistent atlas outputs cannot be exported', async (t) => {
  const input = await fixture(t)
  for (const output of [
    { ...input.output, atlas: undefined },
    {
      ...input.output,
      atlas: { ...input.output.atlas, media: { ...input.reference, name: 'forged.png' } },
    },
    { ...input.output, atlas: { ...input.output.atlas, width: 16 } },
    {
      ...input.output,
      atlas: {
        ...input.output.atlas,
        frames: [{ ...input.output.atlas.frames[0], x: 40 }, input.output.atlas.frames[1]],
      },
    },
    {
      ...input.output,
      atlas: { ...input.output.atlas, media: { ...input.reference, size: 8 * 1024 * 1024 + 1 } },
    },
    {
      ...input.output,
      atlas: { ...input.output.atlas, media: { ...input.reference, mimeType: 'image/jpeg' } },
    },
  ])
    await assert.rejects(exportAgentImages({ ...input, output }), {
      code: 'image_tools_export_invalid',
    })
  await assert.rejects(
    exportAgentImages({
      ...input,
      media: { read: () => Promise.reject(new Error('secret /private/path token')) },
    }),
    { code: 'image_tools_export_failed', message: 'image_tools_export_failed' },
  )
  assert.deepEqual(await readdir(input.cwd), [])
})

test('links in either generated directory level cannot escape the workspace', async (t) => {
  const input = await fixture(t)
  const outside = join(input.root, 'outside')
  await mkdir(outside)
  await writeFile(join(outside, 'keep.txt'), 'keep')
  await symlink(
    outside,
    join(input.cwd, 'generated'),
    process.platform === 'win32' ? 'junction' : 'dir',
  )
  await assert.rejects(exportAgentImages(input), { code: 'image_tools_export_invalid' })
  assert.deepEqual(await readdir(outside), ['keep.txt'])
  await rm(join(input.cwd, 'generated'))
  await mkdir(join(input.cwd, 'generated'))
  await symlink(
    outside,
    join(input.cwd, 'generated', 'image-assets'),
    process.platform === 'win32' ? 'junction' : 'dir',
  )
  await assert.rejects(exportAgentImages(input), { code: 'image_tools_export_invalid' })
  assert.deepEqual(await readdir(outside), ['keep.txt'])
})

test('permission revocation after writing removes only the new directory and retains earlier exports', async (t) => {
  const input = await fixture(t)
  const prior = await exportAgentImages(input)
  let gates = 0
  const plugin = new ImageToolsPlugin({ isAgentEnabled: () => ++gates < 3 })
  await assert.rejects(exportAgentImages({ ...input, plugin }), {
    code: 'image_tools_agent_disabled',
  })
  assert.equal(gates, 3)
  assert.deepEqual(await readFile(prior.files[0].path), input.buffer)
  assert.equal((await readdir(join(input.cwd, 'generated', 'image-assets'))).length, 1)
})

test('cancellation cleans a newly written export while pre-aborted calls have no effects', async (t) => {
  const input = await fixture(t)
  const before = new AbortController()
  before.abort('private reason')
  await assert.rejects(exportAgentImages({ ...input, signal: before.signal }), {
    name: 'AbortError',
    code: 'image_tools_export_cancelled',
    message: 'image_tools_export_cancelled',
  })
  assert.deepEqual(await readdir(input.cwd), [])
  const controller = new AbortController()
  let gates = 0
  const plugin = new ImageToolsPlugin({
    isAgentEnabled: () => {
      if (++gates === 3) controller.abort()
      return true
    },
  })
  await assert.rejects(exportAgentImages({ ...input, plugin, signal: controller.signal }), {
    name: 'AbortError',
    code: 'image_tools_export_cancelled',
  })
  assert.deepEqual(await readdir(join(input.cwd, 'generated', 'image-assets')), [])
})

test('failed writes never overwrite a non-directory path and return no filesystem details', async (t) => {
  const input = await fixture(t)
  const blocked = join(input.cwd, 'generated')
  await writeFile(blocked, 'existing-file')
  await assert.rejects(exportAgentImages(input), {
    code: 'image_tools_export_invalid',
    message: 'image_tools_export_invalid',
  })
  assert.equal(await readFile(blocked, 'utf8'), 'existing-file')
})
