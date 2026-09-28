// ZIP 只解析成受限的内存文件映射，不按归档路径直接解压到磁盘。
import { decodeBoundedZip } from '../storage/bounded-zip.mjs'
import { zipSync } from 'fflate'

export const MAX_WORKFLOW_BUNDLE_BYTES = 128 * 1024 * 1024
const MAX_FILES = 300
const MAX_FILE_BYTES = 64 * 1024 * 1024
const decoder = new TextDecoder('utf-8', { fatal: true })

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
    return decodeBoundedZip(data, {
      maxArchiveBytes: MAX_WORKFLOW_BUNDLE_BYTES,
      maxTotalBytes: MAX_WORKFLOW_BUNDLE_BYTES,
      maxFileBytes: MAX_FILE_BYTES,
      maxFiles: MAX_FILES,
    })
  } catch {
    throw bundleError()
  }
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
