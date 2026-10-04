// 浏览器与 Runtime 在解码前读取尺寸，拒绝压缩体积很小但像素异常巨大的图片。
export const MAX_RASTER_SIDE = 4096
export const MAX_RASTER_PIXELS = 16_000_000

/** @param {Uint8Array} bytes */
export function readRasterDimensions(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  /** @param {number} start @param {number} end */
  const text = (start, end) => String.fromCharCode(...bytes.subarray(start, end))
  if (
    bytes.length >= 24 &&
    bytes[0] === 137 &&
    text(1, 4) === 'PNG' &&
    bytes[4] === 13 &&
    bytes[5] === 10 &&
    bytes[6] === 26 &&
    bytes[7] === 10 &&
    text(12, 16) === 'IHDR'
  )
    return { width: view.getUint32(16), height: view.getUint32(20), mimeType: 'image/png' }
  if (bytes.length >= 30 && text(0, 4) === 'RIFF' && text(8, 12) === 'WEBP') {
    const chunk = text(12, 16)
    /** @param {number} offset */
    const uint24 = (offset) => bytes[offset] + (bytes[offset + 1] << 8) + (bytes[offset + 2] << 16)
    if (chunk === 'VP8X')
      return { width: uint24(24) + 1, height: uint24(27) + 1, mimeType: 'image/webp' }
    if (chunk === 'VP8L' && bytes[20] === 47)
      return {
        width: 1 + bytes[21] + ((bytes[22] & 63) << 8),
        height: 1 + (bytes[22] >> 6) + (bytes[23] << 2) + ((bytes[24] & 15) << 10),
        mimeType: 'image/webp',
      }
    if (chunk === 'VP8 ' && bytes[23] === 157 && bytes[24] === 1 && bytes[25] === 42)
      return {
        width: view.getUint16(26, true) & 16383,
        height: view.getUint16(28, true) & 16383,
        mimeType: 'image/webp',
      }
  }
  if (bytes.length >= 4 && bytes[0] === 255 && bytes[1] === 216) {
    let offset = 2
    while (offset + 4 <= bytes.length) {
      if (bytes[offset] !== 255) return null
      while (bytes[offset] === 255) offset++
      const marker = bytes[offset++]
      if (marker === 217 || marker === 218) return null
      if (marker === 1 || (marker >= 208 && marker <= 215)) continue
      if (offset + 2 > bytes.length) return null
      const size = view.getUint16(offset)
      if (size < 2 || offset + size > bytes.length) return null
      if ([192, 193, 194, 195, 197, 198, 199, 201, 202, 203, 205, 206, 207].includes(marker)) {
        if (size < 8) return null
        return {
          width: view.getUint16(offset + 5),
          height: view.getUint16(offset + 3),
          mimeType: 'image/jpeg',
        }
      }
      offset += size
    }
  }
  return null
}

/** @param {{width:number,height:number}|null} dimensions */
export function assertRasterBounds(dimensions) {
  if (
    !dimensions ||
    !Number.isInteger(dimensions.width) ||
    !Number.isInteger(dimensions.height) ||
    dimensions.width < 1 ||
    dimensions.height < 1
  )
    throw Object.assign(new Error('invalid-image'), { code: 'invalid-image' })
  if (
    dimensions.width > MAX_RASTER_SIDE ||
    dimensions.height > MAX_RASTER_SIDE ||
    dimensions.width * dimensions.height > MAX_RASTER_PIXELS
  )
    throw Object.assign(new Error('image-too-large'), { code: 'image-too-large' })
}
