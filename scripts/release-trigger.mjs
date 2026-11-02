import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

// 触发标签发布：推送 release-<major|minor|patch|X.Y.Z> 标签，由 .github/workflows/release-tag.yml
// 在 CI 里执行 `npm run release -- <参数>`。标签只是触发器，用后即删；版本标签仍由各发布 workflow
// 在产物验证后原子创建，所以触发标签不能复用 v* / *-v* 这些版本标签的命名。
export const RELEASE_TRIGGER_PREFIX = 'release-'

export function releaseInputFromTriggerTag(tag) {
  const value = String(tag || '').trim()
  if (!value.startsWith(RELEASE_TRIGGER_PREFIX)) {
    throw new Error(`触发标签必须以 ${RELEASE_TRIGGER_PREFIX} 开头：${value || '(empty)'}`)
  }
  const input = value.slice(RELEASE_TRIGGER_PREFIX.length)
  if (!['major', 'minor', 'patch'].includes(input) && !/^\d+\.\d+\.\d+$/.test(input)) {
    throw new Error(
      `触发标签无效：${value}。请使用 release-patch、release-minor、release-major 或 release-X.Y.Z。`,
    )
  }
  return input
}

// CI 入口：打印解析出的发布参数，供 workflow 传给 `npm run release`。
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  console.log(releaseInputFromTriggerTag(process.argv[2]))
}
