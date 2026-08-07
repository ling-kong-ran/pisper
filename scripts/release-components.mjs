import { readFile } from 'node:fs/promises'
import { join } from 'node:path'

export const RELEASE_COMPONENTS = Object.freeze({
  desktop: Object.freeze({ tagPrefix: 'v' }),
  tui: Object.freeze({ tagPrefix: 'tui-v' }),
  server: Object.freeze({ tagPrefix: 'server-v' }),
})

export function assertReleaseComponent(value) {
  const component = String(value || '')
    .trim()
    .toLowerCase()
  if (!RELEASE_COMPONENTS[component]) {
    throw new Error(`发布组件无效：${value || '(empty)'}。请使用 desktop、tui 或 server。`)
  }
  return component
}

export function releaseTag(component, version) {
  const normalized = assertReleaseComponent(component)
  return `${RELEASE_COMPONENTS[normalized].tagPrefix}${version}`
}

export function releaseTagPattern(component) {
  const normalized = assertReleaseComponent(component)
  if (normalized === 'desktop') return /^v\d+\.\d+\.\d+$/
  return new RegExp(`^${normalized}-v\\d+\\.\\d+\\.\\d+$`)
}

export function releaseVersionFromTag(component, tag) {
  const normalized = assertReleaseComponent(component)
  if (!releaseTagPattern(normalized).test(tag)) return ''
  return normalized === 'desktop' ? tag.slice(1) : tag.slice(normalized.length + 2)
}

export function fallbackReleaseTag(component, tags) {
  const normalized = assertReleaseComponent(component)
  const values = Array.isArray(tags) ? tags : []
  const current = values.find((tag) => releaseTagPattern(normalized).test(tag))
  if (current) return current
  if (normalized === 'desktop') return ''
  return values.find((tag) => /^v\d+\.\d+\.\d+$/.test(tag)) || ''
}

export async function readComponentVersion(root, component) {
  const normalized = assertReleaseComponent(component)
  if (normalized === 'desktop') {
    return JSON.parse(await readFile(join(root, 'src-tauri', 'desktop-package.json'), 'utf8'))
      .version
  }
  if (normalized === 'server') {
    return JSON.parse(await readFile(join(root, 'package.json'), 'utf8')).version
  }
  const manifest = await readFile(join(root, 'src-tui', 'Cargo.toml'), 'utf8')
  return manifest.match(/\[package\][\s\S]*?\r?\nversion\s*=\s*"([^"]+)"/)?.[1] || ''
}
