import assert from 'node:assert/strict'
import test from 'node:test'
import { readRasterDimensions, assertRasterBounds } from '../../shared/image/raster-image.mjs'
test('sprite header parser handles PNG/JPEG/WebP and rejects unsafe decoded dimensions', () => {
  const png = Buffer.alloc(24)
  png.set([137, 80, 78, 71, 13, 10, 26, 10])
  png.write('IHDR', 12)
  png.writeUInt32BE(512, 16)
  png.writeUInt32BE(1024, 20)
  assert.deepEqual(readRasterDimensions(png), { width: 512, height: 1024, mimeType: 'image/png' })
  const jpeg = Uint8Array.from([255, 216, 255, 194, 0, 8, 8, 0, 32, 0, 64, 1])
  assert.deepEqual(readRasterDimensions(jpeg), { width: 64, height: 32, mimeType: 'image/jpeg' })
  const webp = Buffer.alloc(30)
  webp.write('RIFF')
  webp.write('WEBP', 8)
  webp.write('VP8X', 12)
  webp[24] = 31
  webp[27] = 63
  assert.deepEqual(readRasterDimensions(webp), { width: 32, height: 64, mimeType: 'image/webp' })
  assert.equal(readRasterDimensions(bytes('not an image')), null)
  assert.throws(() => assertRasterBounds({ width: 4096, height: 4096 }), {
    code: 'image-too-large',
  })
  assert.throws(() => assertRasterBounds(null), { code: 'invalid-image' })
})
function bytes(text) {
  return new TextEncoder().encode(text)
}
