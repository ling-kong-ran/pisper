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

export function createStaticHandler(root) {
  const dist = resolve(join(root, 'dist'))
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
    // SPA fallback: unknown paths are served the application shell.
    if (!info?.isFile()) {
      file = join(dist, 'index.html')
      info = await stat(file).catch(() => null)
    }
    if (!info?.isFile()) {
      json(res, 404, { error: '文件不存在。' })
      return
    }
    // Vite emits content-hashed bundles under assets/, safe to cache forever.
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
