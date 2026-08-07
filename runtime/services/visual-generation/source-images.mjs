import { readFile, stat } from 'node:fs/promises'
import { extname, resolve } from 'node:path'

const MAX_SOURCE_IMAGE_BYTES = 24 * 1024 * 1024
const MIME_TYPES = {
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
}

async function loadImage(input, cwd) {
  const path = resolve(cwd, String(input || '').trim())
  const info = await stat(path).catch(() => null)
  if (!info?.isFile()) throw new Error(`编辑源图片不存在：${path}`)
  if (info.size > MAX_SOURCE_IMAGE_BYTES) throw new Error(`编辑源图片超过 24 MB：${path}`)
  const mimeType = MIME_TYPES[extname(path).toLowerCase()]
  if (!mimeType) throw new Error(`图片编辑仅支持 PNG、JPEG 或 WebP：${path}`)
  return { path, mimeType, buffer: await readFile(path) }
}

export async function loadSourceImages(inputs, cwd) {
  const paths = Array.isArray(inputs) ? inputs.filter(Boolean).slice(0, 8) : []
  return Promise.all(paths.map((path) => loadImage(path, cwd)))
}

export async function loadMaskImage(input, cwd) {
  if (!input) return null
  const mask = await loadImage(input, cwd)
  if (mask.mimeType !== 'image/png') throw new Error('图片编辑蒙版必须是 PNG。')
  return mask
}
