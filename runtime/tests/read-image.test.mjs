import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createCompressedReadTool } from '../runtime/pi-coding-agent.mjs'

test('mobile read processes images in-process and preserves the resulting MIME type', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mobile-read-image-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const imagePath = join(directory, 'photo.jpg')
  await writeFile(imagePath, Buffer.from([0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10]))
  let resizeCalls = 0
  const tool = await createCompressedReadTool(directory, {
    runtimeProfile: 'mobile-embedded',
    resizeImageForMobile: async (_buffer, mimeType, options) => {
      resizeCalls += 1
      assert.equal(mimeType, 'image/jpeg')
      assert.deepEqual(options, {
        maxWidth: 1024,
        maxHeight: 1024,
        maxBytes: 1024 * 1024,
      })
      return {
        data: Buffer.from('processed').toString('base64'),
        mimeType: 'image/png',
        originalWidth: 2048,
        originalHeight: 1024,
        width: 1024,
        height: 512,
        wasResized: true,
      }
    },
  })

  const result = await tool.execute('read-photo', { path: imagePath }, undefined, undefined, {
    model: { input: ['text', 'image'] },
  })

  assert.equal(resizeCalls, 1)
  assert.equal(result.content[1].type, 'image')
  assert.equal(result.content[1].mimeType, 'image/png')
  assert.equal(result.content[1].data, Buffer.from('processed').toString('base64'))
  assert.match(result.content[0].text, /original 2048x1024, displayed at 1024x512/)
})

test('mobile read omits an oversized image when in-process decoding fails', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mobile-read-oversized-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const imagePath = join(directory, 'large.jpg')
  const image = Buffer.alloc(800 * 1024, 1)
  image.set([0xff, 0xd8, 0xff, 0xe0])
  await writeFile(imagePath, image)
  const tool = await createCompressedReadTool(directory, {
    runtimeProfile: 'mobile-root',
    resizeImageForMobile: async () => null,
  })

  const result = await tool.execute('read-large-photo', { path: imagePath })

  assert.equal(result.content.length, 1)
  assert.equal(result.content[0].type, 'text')
  assert.match(result.content[0].text, /Image omitted/)
})
