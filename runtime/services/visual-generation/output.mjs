import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

function safeName(value) {
  return String(value || '')
    .trim()
    .replace(/\.[a-z0-9]{2,5}$/i, '')
    .replace(/[^\p{L}\p{N}._-]+/gu, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 80)
}

export async function saveVisualOutput({ cwd, prompt, outputName, result }) {
  const directory = join(cwd, 'generated', 'visuals')
  await mkdir(directory, { recursive: true })
  const stamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19)
  const name = safeName(outputName) || safeName(prompt).slice(0, 42) || 'visual'
  const path = join(directory, `${stamp}-${name}${result.extension}`)
  await writeFile(path, result.buffer)
  return path
}
