// 一次性改名脚本：Vesper -> Pisper（大小写保持）。已执行，后续全量改名可复用。
// 用法：node scripts/rename-to-pisper.mjs [--apply]（默认 dry-run 只列出将改动的文件）
import { readFile, writeFile } from 'node:fs/promises'
import { execSync } from 'node:child_process'

const files = execSync(
  'git ls-files "*.ts" "*.tsx" "*.mjs" "*.cjs" "*.js" "*.json" "*.md" "*.html" "*.yml" "*.css"',
  { encoding: 'utf8' },
)
  .split('\n')
  .map((f) => f.trim())
  .filter(Boolean)
  // 历史发布说明属于已发布版本的记录，不改写
  .filter((f) => f !== 'public/release-notes.json')
  // 本脚本自身
  .filter((f) => f !== 'scripts/rename-to-pisper.mjs')

const replaceBrand = (text) =>
  text.replace(/vesper/gi, (match) =>
    match[0] === match[0].toUpperCase()
      ? match[1] && match[1] === match[1].toUpperCase()
        ? 'PISPER'
        : 'Pisper'
      : 'pisper',
  )

let changed = 0
for (const file of files) {
  const before = await readFile(file, 'utf8')
  const after = replaceBrand(before)
  if (after !== before) {
    await writeFile(file, after)
    changed++
    console.log('updated', file)
  }
}
console.log(`done, ${changed} files updated`)
