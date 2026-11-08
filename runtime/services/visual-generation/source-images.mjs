// 源图片加载：图片编辑（I2V/图生图）的参考图读取，校验存在性/大小/格式。
import { readFile, stat } from 'node:fs/promises'
import { extname, resolve } from 'node:path'
import { pisperError } from '../../../shared/errors.mjs'

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
  if (!info?.isFile())
    throw pisperError('not_found', 'image', `source image ${input}`, { status: 400 })
  if (info.size > MAX_SOURCE_IMAGE_BYTES)
    throw pisperError('too_large', 'image', `max 24 MB: ${input}`)
  const mimeType = MIME_TYPES[extname(path).toLowerCase()]
  if (!mimeType) throw pisperError('unsupported', 'image', `PNG, JPEG or WebP only: ${input}`)
  return { path, mimeType, buffer: await readFile(path) }
}

export async function loadSourceImages(inputs, cwd) {
  const paths = Array.isArray(inputs) ? inputs.filter(Boolean).slice(0, 8) : []
  return Promise.all(paths.map((path) => loadImage(path, cwd)))
}

export async function loadMaskImage(input, cwd) {
  if (!input) return null
  const mask = await loadImage(input, cwd)
  if (mask.mimeType !== 'image/png') throw pisperError('invalid', 'image', 'the mask must be a PNG')
  return mask
}
