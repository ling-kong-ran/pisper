// 静态资源处理器：生产环境托管 dist 构建产物，含目录索引、SPA 回退（未知路径
// 返回应用壳 index.html）与路径穿越防护（禁止访问 dist 之外的文件）。
import { createReadStream } from 'node:fs'
import { stat } from 'node:fs/promises'
import { extname, join, normalize, resolve, sep } from 'node:path'
import { json } from './response.mjs'

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
}

export function createStaticHandler(root, { distRoot } = {}) {
  const dist = resolve(distRoot || join(root, 'dist'))
  return async function serveProduction(_req, res, url) {
    const requested = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, '')
    let file = resolve(dist, requested || 'index.html')
    if (file !== dist && !file.startsWith(`${dist}${sep}`)) {
      json(res, 403, { error: '禁止访问。' })
      return
    }
    let info = await stat(file).catch(() => null)
    if (info?.isDirectory()) {
      file = join(file, 'index.html')
      info = await stat(file).catch(() => null)
    }
    // SPA 回退：未知路径一律返回应用壳 index.html。
    if (!info?.isFile()) {
      file = join(dist, 'index.html')
      info = await stat(file).catch(() => null)
    }
    if (!info?.isFile()) {
      json(res, 404, { error: '文件不存在。' })
      return
    }
    // Vite 的 content-hash 产物位于 assets/ 下，可以无限期缓存。
    const hashed = file.includes(`${sep}assets${sep}`)
    res.writeHead(200, {
      'Content-Type': MIME[extname(file)] || 'application/octet-stream',
      'Content-Length': info.size,
      'Cache-Control': hashed ? 'public, max-age=31536000, immutable' : 'no-cache',
    })
    createReadStream(file)
      .on('error', () => {
        if (!res.headersSent) json(res, 404, { error: '文件不存在。' })
        else res.destroy()
      })
      .pipe(res)
  }
}
