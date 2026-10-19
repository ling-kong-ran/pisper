// 发布前校验已下载的正文；只输出统一标题，工作流直接用于 gh release。
import { readFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { assertReleaseNotes } from './release-notes-format.mjs'

const [path, component, version, tag] = process.argv.slice(2)
if (!path || !component || !version || !tag) {
  throw new Error('用法：node scripts/validate-release-notes.mjs <文件> <渠道> <版本> <标签>')
}
const body = await readFile(path, 'utf8')
const title = assertReleaseNotes(body, {
  component,
  version,
  tag,
  repository: process.env.GITHUB_REPOSITORY || 'ling-kong-ran/pisper',
})
if (component !== 'tui') {
  const manifestPath = join(
    dirname(path),
    component === 'app' ? 'docs/latest-app.json' : 'public/release-notes.json',
  )
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
  const date = body.match(/^- 发布日期：(\d{4}-\d{2}-\d{2})/m)?.[1]
  if (
    manifest.version !== version ||
    manifest.notes !== body.trim() ||
    (component === 'app'
      ? manifest.tag !== tag || manifest.releaseDate !== date
      : manifest.component !== component || manifest.date !== date)
  ) {
    throw new Error('客户端更新清单与 Release 正文不一致，已停止发布。')
  }
}
console.log(title)
