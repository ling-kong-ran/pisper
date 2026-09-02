import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { ROOT } from './screenshot-config.mjs'

const INDEX_PATH = resolve(ROOT, 'docs/index.html')
const SHOW_PATH = resolve(ROOT, 'docs/show.html')
const WEB_SHOT_PATTERN = /shots\/web\/([A-Za-z0-9._-]+)\.webp/g

function referencedShots(source, pattern) {
  return Array.from(source.matchAll(pattern), (match) => match[1])
}

// 官网的展示引用是截图清单的唯一来源，避免已删除的旧场景被验证脚本重新恢复。
const indexSource = readFileSync(INDEX_PATH, 'utf8')
const showSource = readFileSync(SHOW_PATH, 'utf8')
const indexShots = referencedShots(indexSource, /data-shot=["']shots\/web\/([A-Za-z0-9._-]+)\.webp/g)
const showShots = referencedShots(showSource, WEB_SHOT_PATTERN)

export const WEB_SHOTS = [...new Set([...showShots, ...indexShots])]
