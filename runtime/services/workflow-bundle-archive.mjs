// ZIP 只解析成受限的内存文件映射，不按归档路径直接解压到磁盘。
import { inflateRawSync } from 'node:zlib'
import { zipSync } from 'fflate'

export const MAX_WORKFLOW_BUNDLE_BYTES = 128 * 1024 * 1024
const MAX_FILES = 300
const MAX_FILE_BYTES = 64 * 1024 * 1024
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

export function bundleError() {
  return Object.assign(new Error('工作流压缩包无效、包含不支持的文件或超出大小限制。'), {
    code: 'workflow_bundle_invalid',
    statusCode: 400,
  })
}

/** @param {string} name */
function validName(name) {
  return (
    /^[a-zA-Z0-9][a-zA-Z0-9_./-]{0,239}$/.test(name) &&
    !name.split('/').some((part) => part === '..' || part === '.' || part === '')
  )
}

/** @param {Record<string, Uint8Array>} files */
export function encodeWorkflowBundle(files) {
  let total = 0
  const names = new Set()
  const entries = Object.entries(files)
  if (!entries.length || entries.length > MAX_FILES) throw bundleError()
  for (const [name, data] of entries) {
    total += data.byteLength
    if (
      !validName(name) ||
      names.has(name.toLowerCase()) ||
      data.byteLength > MAX_FILE_BYTES ||
      total > MAX_WORKFLOW_BUNDLE_BYTES
    )
      throw bundleError()
    names.add(name.toLowerCase())
  }
  const zipped = Buffer.from(zipSync(files, { level: 1 }))
  if (zipped.byteLength > MAX_WORKFLOW_BUNDLE_BYTES) throw bundleError()
  return zipped
}

/** @param {Uint8Array} data @returns {Record<string, Uint8Array>} */
export function decodeWorkflowBundle(data) {
  try {
    return decodeArchive(data)
  } catch {
    throw bundleError()
  }
}

/** @param {Uint8Array} data @returns {Record<string, Uint8Array>} */
function decodeArchive(data) {
  if (data.byteLength < 22 || data.byteLength > MAX_WORKFLOW_BUNDLE_BYTES) throw bundleError()
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
    throw bundleError()
  const count = view.getUint16(end + 10, true)
  const directorySize = view.getUint32(end + 12, true)
  const directoryStart = view.getUint32(end + 16, true)
  let offset = directoryStart
  if (
    !count ||
    count > MAX_FILES ||
    view.getUint16(end + 8, true) !== count ||
    offset + directorySize !== end
  )
    throw bundleError()
  let total = 0
  const names = new Set()
  /** @type {{name:string,start:number,end:number,size:number,method:number,crc:number}[]} */
  const entries = []
  /** @type {[number, number][]} */
  const ranges = []
  for (let index = 0; index < count; index++) {
    if (offset + 46 > end || view.getUint32(offset, true) !== 0x02014b50) throw bundleError()
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
      throw bundleError()
    const name = decoder.decode(data.subarray(offset + 46, offset + 46 + nameLength))
    total += size
    if (
      !validName(name) ||
      names.has(name.toLowerCase()) ||
      size > MAX_FILE_BYTES ||
      total > MAX_WORKFLOW_BUNDLE_BYTES
    )
      throw bundleError()
    names.add(name.toLowerCase())
    if (
      localOffset + 30 > directoryStart ||
      view.getUint32(localOffset, true) !== 0x04034b50 ||
      view.getUint16(localOffset + 6, true) !== flags ||
      view.getUint16(localOffset + 8, true) !== method
    )
      throw bundleError()
    const localNameLength = view.getUint16(localOffset + 26, true)
    const start = localOffset + 30 + localNameLength + view.getUint16(localOffset + 28, true)
    if (
      start + compressed > directoryStart ||
      decoder.decode(data.subarray(localOffset + 30, localOffset + 30 + localNameLength)) !== name
    )
      throw bundleError()
    if (
      !(flags & 8) &&
      (view.getUint32(localOffset + 14, true) !== crc ||
        view.getUint32(localOffset + 18, true) !== compressed ||
        view.getUint32(localOffset + 22, true) !== size)
    )
      throw bundleError()
    ranges.push([localOffset, start + compressed])
    entries.push({ name, start, end: start + compressed, size, method, crc })
    offset = next
  }
  if (offset !== end) throw bundleError()
  ranges.sort((a, b) => a[0] - b[0])
  if (ranges.some((range, index) => index > 0 && range[0] < ranges[index - 1][1]))
    throw bundleError()
  /** @type {Record<string, Uint8Array>} */
  const files = {}
  for (const entry of entries) {
    const compressed = data.subarray(entry.start, entry.end)
    // 原生解压器对实际输出施加上限，不能仅相信可伪造的 ZIP 目录尺寸。
    const content =
      entry.method === 0
        ? Uint8Array.from(compressed)
        : inflateRawSync(compressed, { maxOutputLength: Math.max(1, entry.size) })
    if (content.byteLength !== entry.size || checksum(content) !== entry.crc) throw bundleError()
    files[entry.name] = content
  }
  return files
}

/** @param {Record<string, Uint8Array>} files @param {string} name @returns {unknown} */
export function bundleJson(files, name) {
  const bytes = files[name]
  if (!bytes || bytes.byteLength > 16 * 1024 * 1024) throw bundleError()
  try {
    return JSON.parse(decoder.decode(bytes))
  } catch {
    throw bundleError()
  }
}

/** @param {unknown} value */
export function jsonBundleFile(value) {
  return new TextEncoder().encode(JSON.stringify(value, null, 2))
}
