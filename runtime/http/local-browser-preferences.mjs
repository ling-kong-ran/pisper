import {
  BROWSER_PREFERENCE_MAX_TOTAL_BYTES,
  BrowserPreferenceError,
} from '../../shared/browser-preferences.mjs'
import { json } from './response.mjs'

const PATH = '/api/local/browser-preferences'
// JSON.stringify 最坏会将单字节控制字符写成六字节的 \u00xx；另留键名、修订号和结构开销。
const MAX_BODY_BYTES = BROWSER_PREFERENCE_MAX_TOTAL_BYTES * 6 + 16 * 1024

/** @param {import('node:http').IncomingMessage} req */
async function readUpdates(req) {
  /** @type {Buffer[]} */
  const chunks = []
  let size = 0
  for await (const chunk of req) {
    size += chunk.length
    if (size > MAX_BODY_BYTES) throw new BrowserPreferenceError()
    chunks.push(chunk)
  }
  try {
    return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks)))
  } catch {
    throw new BrowserPreferenceError()
  }
}

/**
 * 本机偏好走移动代理保留的 /api/local 路径，远程模式仍由嵌入式 Runtime 处理。
 * 请求已经经过 app-runtime 的桌面 Cookie / Origin 鉴权，远程监听不开放此入口。
 * @param {import('node:http').IncomingMessage} req
 * @param {import('node:http').ServerResponse} res
 * @param {URL} url
 * @param {{ snapshot: () => Promise<unknown>, update: (value: unknown, revisions?: unknown) => Promise<void> }} service
 * @param {{ remote: boolean, origin: string }} options
 */
export async function handleLocalBrowserPreferences(req, res, url, service, { remote, origin }) {
  if (url.pathname !== PATH) return false
  if (remote) {
    json(res, 404, { code: 'browser_preferences_unavailable' })
    return true
  }
  if (req.method === 'GET') {
    try {
      json(res, 200, await service.snapshot())
    } catch {
      json(res, 500, { code: 'browser_preferences_unavailable' })
    }
    return true
  }
  if (req.method !== 'PUT' && req.method !== 'POST') {
    json(res, 405, { code: 'browser_preferences_method' })
    return true
  }
  if (
    req.headers.origin &&
    req.headers.origin !== origin &&
    req.headers.origin !== origin.replace('127.0.0.1', 'localhost')
  ) {
    json(res, 403, { code: 'browser_preferences_origin' })
    return true
  }
  try {
    const value = await readUpdates(req)
    if (
      !value ||
      typeof value !== 'object' ||
      Array.isArray(value) ||
      Object.keys(value).some((key) => key !== 'updates' && key !== 'revisions') ||
      !('updates' in value)
    )
      throw new BrowserPreferenceError()
    await service.update(value.updates, value.revisions)
    res.writeHead(204, { 'Cache-Control': 'no-store' })
    res.end()
  } catch (error) {
    const invalid = error instanceof BrowserPreferenceError
    json(res, invalid ? 400 : 500, {
      code: invalid ? 'browser_preferences_invalid' : 'browser_preferences_storage_failed',
    })
  }
  return true
}
