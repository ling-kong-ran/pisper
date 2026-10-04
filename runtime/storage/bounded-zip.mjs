// 受限 ZIP 读取机制：先核对目录与本地记录，再按声明尺寸限制实际解压；不向磁盘写入归档路径。
import { inflateRawSync } from 'node:zlib'

/** @typedef {{maxArchiveBytes:number,maxTotalBytes:number,maxFileBytes:number,maxFiles:number,maxPathLength?:number,allowDirectories?:boolean}} ZipLimits */
export class BoundedZipError extends Error {
  /** @param {string} [code] */
  constructor(code = 'zip_archive_invalid') {
    super(code)
    this.code = code
  }
}

/** @param {string} name @param {ZipLimits} limits */
function validName(name, limits) {
  const path = name.endsWith('/') && limits.allowDirectories ? name.slice(0, -1) : name
  return (
    name.length <= (limits.maxPathLength ?? 240) &&
    /^[a-zA-Z0-9][a-zA-Z0-9_./-]*$/.test(path) &&
    !path.split('/').some((part) => !part || part === '.' || part === '..')
  )
}

/** @param {Uint8Array} data @param {ZipLimits} limits @returns {Record<string, Uint8Array>} */
export function decodeBoundedZip(data, limits) {
  try {
    return decodeArchive(data, limits)
  } catch (error) {
    if (error instanceof BoundedZipError) throw error
    throw new BoundedZipError()
  }
}

const decoder = new TextDecoder('utf-8', { fatal: true })
const crcTable = Uint32Array.from({ length: 256 }, (_, index) => {
  let value = index
  for (let bit = 0; bit < 8; bit++) value = (value >>> 1) ^ (value & 1 ? 0xedb88320 : 0)
  return value >>> 0
})

/** @param {Uint8Array} data */
function checksum(data) {
  let value = 0xffffffff
  for (const byte of data) value = (value >>> 8) ^ crcTable[(value ^ byte) & 255]
  return (value ^ 0xffffffff) >>> 0
}

/** @param {Uint8Array} data @param {ZipLimits} limits @returns {Record<string, Uint8Array>} */
function decodeArchive(data, limits) {
  if (data.byteLength < 22 || data.byteLength > limits.maxArchiveBytes) throw new BoundedZipError()
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength)
  let end = data.byteLength - 22
  for (; end >= Math.max(0, data.byteLength - 65557); end--) {
    if (
      view.getUint32(end, true) === 0x06054b50 &&
      end + 22 + view.getUint16(end + 20, true) === data.byteLength
    )
      break
  }
  if (
    end < 0 ||
    view.getUint32(end, true) !== 0x06054b50 ||
    view.getUint16(end + 4, true) !== 0 ||
    view.getUint16(end + 6, true) !== 0
  )
    throw new BoundedZipError()
  const count = view.getUint16(end + 10, true)
  const directorySize = view.getUint32(end + 12, true)
  const directoryStart = view.getUint32(end + 16, true)
  let offset = directoryStart
  if (
    !count ||
    count > limits.maxFiles ||
    view.getUint16(end + 8, true) !== count ||
    offset + directorySize !== end
  )
    throw new BoundedZipError()
  let total = 0
  const names = new Set()
  /** @type {{name:string,start:number,end:number,size:number,method:number,crc:number}[]} */
  const entries = []
  /** @type {[number, number][]} */
  const ranges = []
  for (let index = 0; index < count; index++) {
    if (offset + 46 > end || view.getUint32(offset, true) !== 0x02014b50)
      throw new BoundedZipError()
    const flags = view.getUint16(offset + 8, true)
    const method = view.getUint16(offset + 10, true)
    const crc = view.getUint32(offset + 16, true)
    const compressed = view.getUint32(offset + 20, true)
    const size = view.getUint32(offset + 24, true)
    const nameLength = view.getUint16(offset + 28, true)
    const extraLength = view.getUint16(offset + 30, true)
    const commentLength = view.getUint16(offset + 32, true)
    const mode = view.getUint32(offset + 38, true) >>> 16
    const localOffset = view.getUint32(offset + 42, true)
    const next = offset + 46 + nameLength + extraLength + commentLength
    if (
      next > end ||
      !nameLength ||
      flags & ~0x808 ||
      ![0, 8].includes(method) ||
      (mode & 0xf000) === 0xa000 ||
      view.getUint16(offset + 34, true) !== 0
    )
      throw new BoundedZipError()
    const name = decoder.decode(data.subarray(offset + 46, offset + 46 + nameLength))
    const directory = name.endsWith('/')
    const canonicalName = directory ? name.slice(0, -1) : name
    total += size
    if (size > limits.maxFileBytes || total > limits.maxTotalBytes)
      throw new BoundedZipError('zip_archive_too_large')
    if (
      !validName(name, limits) ||
      names.has(canonicalName.toLowerCase()) ||
      (directory && (!limits.allowDirectories || size !== 0 || crc !== 0))
    )
      throw new BoundedZipError()
    names.add(canonicalName.toLowerCase())
    if (
      localOffset + 30 > directoryStart ||
      view.getUint32(localOffset, true) !== 0x04034b50 ||
      view.getUint16(localOffset + 6, true) !== flags ||
      view.getUint16(localOffset + 8, true) !== method
    )
      throw new BoundedZipError()
    const localNameLength = view.getUint16(localOffset + 26, true)
    const start = localOffset + 30 + localNameLength + view.getUint16(localOffset + 28, true)
    if (
      start + compressed > directoryStart ||
      decoder.decode(data.subarray(localOffset + 30, localOffset + 30 + localNameLength)) !== name
    )
      throw new BoundedZipError()
    if (
      !(flags & 8) &&
      (view.getUint32(localOffset + 14, true) !== crc ||
        view.getUint32(localOffset + 18, true) !== compressed ||
        view.getUint32(localOffset + 22, true) !== size)
    )
      throw new BoundedZipError()
    ranges.push([localOffset, start + compressed])
    entries.push({ name, start, end: start + compressed, size, method, crc })
    offset = next
  }
  if (offset !== end) throw new BoundedZipError()
  ranges.sort((a, b) => a[0] - b[0])
  if (ranges.some((range, index) => index > 0 && range[0] < ranges[index - 1][1]))
    throw new BoundedZipError()
  /** @type {Record<string, Uint8Array>} */
  const files = {}
  for (const entry of entries) {
    const compressed = data.subarray(entry.start, entry.end)
    // 原生解压器对实际输出施加上限，不能仅相信可伪造的 ZIP 目录尺寸。
    const content =
      entry.method === 0
        ? Uint8Array.from(compressed)
        : inflateRawSync(compressed, { maxOutputLength: Math.max(1, entry.size) })
    if (content.byteLength !== entry.size || checksum(content) !== entry.crc)
      throw new BoundedZipError()
    if (!entry.name.endsWith('/')) files[entry.name] = content
  }
  return files
}
