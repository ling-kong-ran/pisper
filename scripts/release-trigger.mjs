import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

// 触发标签发布，两条路径：
// - 推送 release-<major|minor|patch|X.Y.Z>：.github/workflows/release-tag.yml 在 GitHub 托管 Runner 上
//   执行 `npm run release -- <参数>`；
// - 推送 self-release-<…>：.github/workflows/release-self-hosted-tag.yml 全程在自托管 Runner 上走本机发布脚本
//   （release-local.mjs 的 plan → 三个平台 build → publish），托管额度用尽时用这条。
// 标签只是触发器，用后即删；版本标签仍由发布流程在产物验证后创建，所以触发标签不能复用 v* / *-v* 这些版本标签的命名。
export const RELEASE_TRIGGER_PREFIX = 'release-'
export const SELF_HOSTED_TRIGGER_PREFIX = 'self-release-'

export function releaseInputFromTriggerTag(tag, prefix = RELEASE_TRIGGER_PREFIX) {
  const value = String(tag || '').trim()
  if (!value.startsWith(prefix)) {
    throw new Error(`触发标签必须以 ${prefix} 开头：${value || '(empty)'}`)
  }
  const input = value.slice(prefix.length)
  if (!['major', 'minor', 'patch'].includes(input) && !/^\d+\.\d+\.\d+$/.test(input)) {
    throw new Error(
      `触发标签无效：${value}。请使用 ${prefix}patch、${prefix}minor、${prefix}major 或 ${prefix}X.Y.Z。`,
    )
  }
  return input
}

// CI 入口：打印解析出的发布参数。第二个参数 --self-hosted 表示按 self-release- 前缀解析。
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const prefix =
    process.argv[3] === '--self-hosted' ? SELF_HOSTED_TRIGGER_PREFIX : RELEASE_TRIGGER_PREFIX
  console.log(releaseInputFromTriggerTag(process.argv[2], prefix))
}
