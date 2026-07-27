import { createHash } from 'node:crypto'
import { createReadStream, createWriteStream } from 'node:fs'
import { mkdir, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'

const MAX_REDIRECTS = 10
const RETRY_DELAYS_MS = [1_000, 3_000]

function headerValue(headers, name) {
  if (!headers) return ''
  if (typeof headers.get === 'function') return String(headers.get(name) || '')
  const key = Object.keys(headers).find((item) => item.toLowerCase() === name.toLowerCase())
  const value = key ? headers[key] : ''
  return String(Array.isArray(value) ? value.at(-1) || '' : value || '')
}

function setDefaultHeader(headers, name, value) {
  if (!Object.keys(headers).some((item) => item.toLowerCase() === name.toLowerCase())) headers[name] = value
}

function requestOptions(url, headers) {
  const parsed = new URL(url)
  const nextHeaders = { ...(headers || {}) }
  setDefaultHeader(nextHeaders, 'User-Agent', 'Pisper Updater')
  setDefaultHeader(nextHeaders, 'Cache-Control', 'no-cache')
  return {
    protocol: parsed.protocol,
    hostname: parsed.hostname,
    ...(parsed.port ? { port: parsed.port } : {}),
    path: `${parsed.pathname}${parsed.search}`,
    headers: nextHeaders,
    redirect: 'manual',
  }
}

function openElectronResponse(executor, url, headers) {
  return new Promise((resolve, reject) => {
    let settled = false

    const fail = (error) => {
      if (settled) return
      settled = true
      reject(error instanceof Error ? error : new Error(String(error)))
    }

    const open = (options, redirectCount = 0) => {
      if (redirectCount > MAX_REDIRECTS) return fail(new Error(`Too many update download redirects (>${MAX_REDIRECTS}).`))
      const request = executor.createRequest(options, (response) => {
        if (settled) return
        settled = true
        resolve({
          statusCode: Number(response.statusCode) || 0,
          headers: response.headers,
          body: response,
          abort: () => request.abort(),
        })
      })
      executor.addErrorAndTimeoutHandlers(request, fail)
      executor.addRedirectHandlers(request, options, fail, redirectCount, (redirectedOptions) => {
        open(redirectedOptions, redirectCount + 1)
      })
      request.end()
    }

    open(requestOptions(url, headers))
  })
}

function parseContentRange(value) {
  const complete = String(value || '').match(/^bytes\s+(\d+)-(\d+)\/(\d+|\*)$/i)
  if (complete) return { start: Number(complete[1]), end: Number(complete[2]), total: complete[3] === '*' ? 0 : Number(complete[3]) }
  const unsatisfied = String(value || '').match(/^bytes\s+\*\/(\d+)$/i)
  return unsatisfied ? { start: -1, end: -1, total: Number(unsatisfied[1]) } : null
}

async function fileSize(path) {
  try { return (await stat(path)).size } catch (error) { if (error?.code === 'ENOENT') return 0; throw error }
}

async function readMetadata(path) {
  try { return JSON.parse(await readFile(path, 'utf8')) } catch { return {} }
}

async function digestFile(path, algorithm, encoding) {
  const hash = createHash(algorithm)
  for await (const chunk of createReadStream(path)) hash.update(chunk)
  return hash.digest(encoding)
}

function expectedDigest(options) {
  if (options.sha512) {
    const value = String(options.sha512)
    const encoding = value.length === 128 && !/[+Z=]/.test(value) ? 'hex' : 'base64'
    return { algorithm: 'sha512', encoding, value }
  }
  if (options.sha2) return { algorithm: 'sha256', encoding: 'hex', value: String(options.sha2) }
  return null
}

async function verifyDownload(path, options) {
  const expected = expectedDigest(options)
  if (!expected) return true
  const actual = await digestFile(path, expected.algorithm, expected.encoding)
  return actual === expected.value
}

function downloadIdentity(url, options) {
  const expected = expectedDigest(options)?.value || ''
  return createHash('sha256').update(String(url)).update('\0').update(expected).digest('hex')
}

async function cleanOtherDownloads(directory, keepNames) {
  const entries = await readdir(directory).catch(() => [])
  await Promise.allSettled(entries.filter((name) => !keepNames.has(name)).map((name) => rm(join(directory, name), { recursive: true, force: true })))
}

async function resetPartial(partPath, metadataPath) {
  await Promise.allSettled([rm(partPath, { force: true }), rm(metadataPath, { force: true })])
}

async function promotePartial(partPath, destination, metadataPath) {
  await rm(destination, { force: true })
  await rename(partPath, destination)
  await rm(metadataPath, { force: true })
  return destination
}

function createProgressTransform({ start, total, cancellationToken, onProgress }) {
  let transferred = start
  let delta = 0
  let nextUpdate = Date.now()
  const startedAt = Date.now()
  const emit = (force = false) => {
    const now = Date.now()
    if (!force && now < nextUpdate) return
    nextUpdate = now + 500
    onProgress?.({
      total,
      delta,
      transferred,
      percent: total > 0 ? Math.min(100, (transferred / total) * 100) : 0,
      bytesPerSecond: Math.round(Math.max(0, transferred - start) / Math.max(0.001, (now - startedAt) / 1_000)),
    })
    delta = 0
  }
  emit(true)
  return new Transform({
    transform(chunk, _encoding, callback) {
      if (cancellationToken?.cancelled) return callback(new Error('cancelled'))
      transferred += chunk.length
      delta += chunk.length
      emit(false)
      callback(null, chunk)
    },
    flush(callback) {
      emit(true)
      callback()
    },
  })
}

export async function downloadResumableFile({
  url,
  destination,
  options = {},
  resumeDirectory = join(dirname(dirname(destination)), 'resume'),
  openResponse,
  logger = console,
}) {
  await mkdir(dirname(destination), { recursive: true })
  await mkdir(resumeDirectory, { recursive: true })
  const identity = downloadIdentity(url, options)
  const partName = `${identity}.part`
  const metadataName = `${identity}.json`
  const partPath = join(resumeDirectory, partName)
  const metadataPath = join(resumeDirectory, metadataName)
  await cleanOtherDownloads(resumeDirectory, new Set([partName, metadataName]))

  let metadata = await readMetadata(metadataPath)
  let start = await fileSize(partPath)
  if (metadata.total > 0 && start === metadata.total) {
    if (await verifyDownload(partPath, options)) return promotePartial(partPath, destination, metadataPath)
    await resetPartial(partPath, metadataPath)
    metadata = {}
    start = 0
  } else if (metadata.total > 0 && start > metadata.total) {
    await resetPartial(partPath, metadataPath)
    metadata = {}
    start = 0
  }

  for (let restart = 0; restart < 2; restart += 1) {
    const headers = { ...(options.headers || {}) }
    if (start > 0) {
      headers.Range = `bytes=${start}-`
      const validator = metadata.etag && !String(metadata.etag).startsWith('W/') ? metadata.etag : metadata.lastModified
      if (validator) headers['If-Range'] = validator
      logger.info('Resuming update download.', { downloadedBytes: start })
    }

    const response = await openResponse({ url, headers })
    const contentRange = parseContentRange(headerValue(response.headers, 'content-range'))
    const contentLength = Number(headerValue(response.headers, 'content-length')) || 0

    if (response.statusCode === 416) {
      response.abort?.()
      if (start > 0 && contentRange?.total === start && await verifyDownload(partPath, options)) {
        return promotePartial(partPath, destination, metadataPath)
      }
      await resetPartial(partPath, metadataPath)
      metadata = {}
      start = 0
      continue
    }
    if (response.statusCode >= 400) {
      response.abort?.()
      const error = new Error(`Update download failed with HTTP ${response.statusCode}.`)
      error.retryable = response.statusCode === 408 || response.statusCode === 429 || response.statusCode >= 500
      throw error
    }

    let append = false
    if (start > 0 && response.statusCode === 206) {
      if (!contentRange || contentRange.start !== start) {
        response.abort?.()
        await resetPartial(partPath, metadataPath)
        throw new Error('Update server returned an invalid resume range.')
      }
      append = true
    } else if (start > 0) {
      logger.warn('Update server ignored the resume range; restarting this file from zero.')
      start = 0
      metadata = {}
    }

    const total = contentRange?.total || (append ? start + contentLength : contentLength)
    metadata = {
      total,
      etag: headerValue(response.headers, 'etag'),
      lastModified: headerValue(response.headers, 'last-modified'),
    }
    await writeFile(metadataPath, `${JSON.stringify(metadata)}\n`, 'utf8')

    let cancelHandler = null
    try {
      cancelHandler = () => response.abort?.()
      options.cancellationToken?.onCancel?.(cancelHandler)
      await pipeline(
        response.body,
        createProgressTransform({ start, total, cancellationToken: options.cancellationToken, onProgress: options.onProgress }),
        createWriteStream(partPath, { flags: append ? 'a' : 'w' }),
      )
    } catch (error) {
      const downloadedBytes = await fileSize(partPath)
      logger.warn('Update download was interrupted; partial data was kept for resuming.', { downloadedBytes })
      throw error
    } finally {
      if (cancelHandler) options.cancellationToken?.removeListener?.('cancel', cancelHandler)
    }

    const downloadedBytes = await fileSize(partPath)
    if (total > 0 && downloadedBytes !== total) {
      throw new Error(`Update download ended early (${downloadedBytes}/${total} bytes).`)
    }
    if (!await verifyDownload(partPath, options)) {
      await resetPartial(partPath, metadataPath)
      const error = new Error('Downloaded update checksum does not match the release metadata.')
      error.retryable = true
      throw error
    }
    logger.info('Resumable update download completed.', { downloadedBytes })
    return promotePartial(partPath, destination, metadataPath)
  }

  throw new Error('Update download could not resume because the remote file changed.')
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

export async function downloadResumableWithRetry(input, { retryDelays = RETRY_DELAYS_MS } = {}) {
  let lastError = null
  for (let attempt = 0; attempt <= retryDelays.length; attempt += 1) {
    try {
      return await downloadResumableFile(input)
    } catch (error) {
      lastError = error
      if (input.options?.cancellationToken?.cancelled || error?.retryable === false || attempt >= retryDelays.length) throw error
      const waitMs = retryDelays[attempt]
      input.logger?.warn?.('Retrying interrupted update download.', { attempt: attempt + 2, waitMs, message: error instanceof Error ? error.message : String(error) })
      await delay(waitMs)
    }
  }
  throw lastError
}

export function enableResumableUpdateDownloads(autoUpdater, { logger = console } = {}) {
  const executor = autoUpdater?.httpExecutor
  if (!executor || executor.__pisperResumableDownload) return false
  executor.__pisperResumableDownload = true
  executor.download = async (url, destination, options) => {
    return downloadResumableWithRetry({
      url,
      destination,
      options,
      logger,
      openResponse: ({ url: requestUrl, headers }) => openElectronResponse(executor, requestUrl, headers),
    })
  }
  return true
}
