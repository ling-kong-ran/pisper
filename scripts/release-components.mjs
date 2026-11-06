import { readFile } from 'node:fs/promises'
import { join } from 'node:path'

export const RELEASE_COMPONENTS = Object.freeze({
  desktop: Object.freeze({ tagPrefix: 'v' }),
  tui: Object.freeze({ tagPrefix: 'tui-v' }),
  runtime: Object.freeze({ tagPrefix: 'runtime-v' }),
})

export function assertReleaseComponent(value) {
  const component = String(value || '')
    .trim()
    .toLowerCase()
  if (!RELEASE_COMPONENTS[component]) {
    throw new Error(`发布组件无效：${value || '(empty)'}。请使用 desktop、tui 或 runtime。`)
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

export function fallbackReleaseTag(component, tags, currentVersion = '') {
  const normalized = assertReleaseComponent(component)
  const values = Array.isArray(tags) ? tags : []
  const current = values.find((tag) => releaseTagPattern(normalized).test(tag))
  if (current) return current
  if (normalized === 'desktop') return ''

  const legacyTags = values
    .filter((tag) => /^v\d+\.\d+\.\d+$/.test(tag))
    .sort((left, right) => {
      const a = left.slice(1).split('.').map(Number)
      const b = right.slice(1).split('.').map(Number)
      for (let index = 0; index < 3; index += 1) {
        if (a[index] !== b[index]) return b[index] - a[index]
      }
      return 0
    })
  if (!currentVersion) return legacyTags[0] || ''
  const limit = currentVersion.split('.').map(Number)
  return (
    legacyTags.find((tag) => {
      const version = tag.slice(1).split('.').map(Number)
      for (let index = 0; index < 3; index += 1) {
        if (version[index] !== limit[index]) return version[index] < limit[index]
      }
      return true
    }) || ''
  )
}

// Desktop 安装包内置的组件版本：同批次发布的用新版本，否则用最新标签，和构建时
// fetch-bundled-components 实际取的一致。只按同批次写的话，重新触发或只改界面的发布
// 不带这两个值，提交里的 bundled 就一直停在旧版本。
export function bundledComponentVersion(component, plans, tags) {
  return (
    plans.find((plan) => plan.component === component)?.nextVersion ||
    releaseVersionFromTag(component, fallbackReleaseTag(component, tags))
  )
}

export async function readComponentVersion(root, component) {
  const normalized = assertReleaseComponent(component)
  if (normalized === 'desktop') {
    return JSON.parse(await readFile(join(root, 'src-tauri', 'desktop-package.json'), 'utf8'))
      .version
  }
  if (normalized === 'runtime') {
    return JSON.parse(await readFile(join(root, 'package.json'), 'utf8')).version
  }
  const manifest = await readFile(join(root, 'src-tui', 'Cargo.toml'), 'utf8')
  return manifest.match(/\[package\][\s\S]*?\r?\nversion\s*=\s*"([^"]+)"/)?.[1] || ''
}
