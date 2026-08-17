import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('asset cards grow with their copy instead of clipping the action row', async () => {
  const source = await readFile('src/features/assets/AssetsPage.tsx', 'utf8')

  // 行高随文字内容增长，截图高度不再决定行高
  assert.match(source, /asset-grid[^"\n]*auto-rows-\[minmax\(292px,auto\)\]/)
  // 卡片填充整行但只做最小高度约束，内容超出时不会裁剪底部操作
  assert.match(source, /asset-card[^"\n]*h-full min-h-\[292px\][^"\n]*overflow-hidden/)
  // 预览固定高度且不可压缩，操作行同样不可纵向压缩
  assert.match(source, /asset-preview[^`]*h-44[^`]*shrink-0/)
  assert.match(source, /asset-card-actions flex-none/)
})
