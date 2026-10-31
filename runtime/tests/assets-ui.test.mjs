import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('asset page exposes a dedicated video filter', async () => {
  const [source, api] = await Promise.all([
    readFile('src/features/assets/AssetsPage.tsx', 'utf8'),
    readFile('src/features/assets/assets-api.ts', 'utf8'),
  ])

  assert.match(source, /KINDS[^\n]*'image', 'video', 'file'/)
  assert.match(source, /kind === 'video'[^\n]*assetsPage\.videos/)
  // Runtime 把视频登记为 file，视频筛选在 API 边界按 MIME 派生。
  assert.match(api, /params\.set\('kind', kind === 'video' \? 'file' : kind\)/)
  assert.match(api, /kind === 'video'[\s\S]*?mimeType\?\.startsWith\('video\/'\)/)
})

test('asset rows grow with their copy and keep every action reachable on narrow screens', async () => {
  const source = await readFile('src/features/assets/AssetsPage.tsx', 'utf8')

  // A6 第 2 批：卡片墙改为功能页模板的列表行（规格 10 A6、6.4）。行高随内容（模板行只有最小高度），
  // 名称区就是“预览”按钮，缩略图只是装饰；不再有固定高度的预览区去挤压操作行。
  assert.match(source, /<FeatureList>\s*\{assets\.map\(\(asset\) => \(\s*<AssetRow/)
  assert.match(source, /onOpen=\{\(event\) => onPreview\(event\.currentTarget\)\}/)
  assert.match(source, /openLabel=\{`\$\{t\('assets:assetsPage\.preview'\)\}: \$\{asset\.name\}`\}/)
  assert.doesNotMatch(source, /asset-grid|AppCard|h-44/)
  // 手机宽度：行内按钮只留图标并抬到 44，文字留给读屏（sr-only），名称不被挤掉、操作不被裁掉。
  const actions = source.slice(source.indexOf('asset-row-actions'))
  assert.equal(actions.match(/max-\[650px\]:sr-only/g)?.length, 3)
  assert.equal(actions.match(/max-\[650px\]:size-11/g)?.length, 4)
})
