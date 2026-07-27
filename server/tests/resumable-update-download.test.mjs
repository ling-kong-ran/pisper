import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Readable } from 'node:stream'
import test from 'node:test'
import { downloadResumableFile, downloadResumableWithRetry, enableResumableUpdateDownloads } from '../../electron/resumable-update-download.mjs'

function sha512(buffer) {
  return createHash('sha512').update(buffer).digest('base64')
}

function downloadIdentity(url, options) {
  return createHash('sha256').update(String(url)).update('\0').update(String(options.sha512 || '')).digest('hex')
}

function response({ statusCode = 200, headers = {}, body }) {
  return { statusCode, headers, body: Readable.from(body), abort() {} }
}

async function fixture(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-resumable-update-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  return {
    directory,
    destination: join(directory, 'pending', 'temp-update.exe'),
    resumeDirectory: join(directory, 'resume'),
  }
}

async function seedPartial(paths, { url, options, content, split, etag }) {
  const identity = downloadIdentity(url, options)
  await mkdir(paths.resumeDirectory, { recursive: true })
  const partPath = join(paths.resumeDirectory, `${identity}.part`)
  const metadataPath = join(paths.resumeDirectory, `${identity}.json`)
  await writeFile(partPath, content.subarray(0, split))
  await writeFile(metadataPath, `${JSON.stringify({
    total: content.length,
    ...(etag ? { etag } : {}),
  })}\n`)
  return { partPath, metadataPath }
}

test('interrupted update downloads resume from the persisted byte offset', async (t) => {
  const paths = await fixture(t)
  const content = Buffer.alloc(1024 * 1024, 7)
  const split = 410_321
  const url = 'https://example.test/Pisper.exe'
  const options = { sha512: sha512(content) }
  await seedPartial(paths, { url, options, content, split, etag: '"release-1"' })

  let requests = 0
  let resumedRange = ''
  await downloadResumableFile({
    url,
    ...paths,
    options,
    openResponse: async ({ headers }) => {
      requests += 1
      resumedRange = headers.Range
      return response({
        statusCode: 206,
        headers: {
          'content-length': String(content.length - split),
          'content-range': `bytes ${split}-${content.length - 1}/${content.length}`,
          etag: '"release-1"',
        },
        body: [content.subarray(split)],
      })
    },
  })

  assert.equal(requests, 1)
  assert.equal(resumedRange, `bytes=${split}-`)
  assert.deepEqual(await readFile(paths.destination), content)
  assert.deepEqual(await readdir(paths.resumeDirectory), [])
})

test('a server that ignores Range safely restarts the current file from zero', async (t) => {
  const paths = await fixture(t)
  const content = Buffer.from('complete installer payload')
  const split = 8
  const url = 'https://example.test/Pisper.exe'
  const options = { sha512: sha512(content) }
  await seedPartial(paths, { url, options, content, split })

  let requests = 0
  let receivedRange = ''
  await downloadResumableFile({
    url,
    ...paths,
    options,
    openResponse: async ({ headers }) => {
      requests += 1
      receivedRange = headers.Range
      // 200 without content-range: resume range ignored, full body from zero.
      return response({ headers: { 'content-length': String(content.length) }, body: [content] })
    },
  })

  assert.equal(requests, 1)
  assert.equal(receivedRange, `bytes=${split}-`)
  assert.deepEqual(await readFile(paths.destination), content)
})

test('transient failures retry automatically using the saved partial file', async (t) => {
  const paths = await fixture(t)
  const content = Buffer.alloc(64 * 1024, 3)
  const split = 12_345
  const url = 'https://example.test/Pisper.exe'
  const options = { sha512: sha512(content) }
  await seedPartial(paths, { url, options, content, split })
  const ranges = []
  let requests = 0

  await downloadResumableWithRetry({
    url,
    ...paths,
    options,
    openResponse: async ({ headers }) => {
      requests += 1
      ranges.push(headers.Range || '')
      if (requests === 1) {
        const body = new Readable({
          read() {
            this.destroy(new Error('temporary disconnect'))
          },
        })
        return {
          statusCode: 206,
          headers: {
            'content-length': String(content.length - split),
            'content-range': `bytes ${split}-${content.length - 1}/${content.length}`,
          },
          body,
          abort: () => body.destroy(),
        }
      }
      return response({
        statusCode: 206,
        headers: {
          'content-length': String(content.length - split),
          'content-range': `bytes ${split}-${content.length - 1}/${content.length}`,
        },
        body: [content.subarray(split)],
      })
    },
  }, { retryDelays: [0] })

  assert.deepEqual(ranges, [`bytes=${split}-`, `bytes=${split}-`])
  assert.deepEqual(await readFile(paths.destination), content)
})

test('checksum failures remove unusable partial update data', async (t) => {
  const paths = await fixture(t)
  const expected = Buffer.from('expected installer')
  const corrupted = Buffer.from('corrupted installer')

  await assert.rejects(downloadResumableFile({
    url: 'https://example.test/Pisper.exe',
    ...paths,
    options: { sha512: sha512(expected) },
    openResponse: async () => response({
      headers: { 'content-length': String(corrupted.length) },
      body: [corrupted],
    }),
  }), /checksum/)

  assert.deepEqual(await readdir(paths.resumeDirectory), [])
})

test('the Electron updater executor is replaced without changing its download contract', async (t) => {
  const paths = await fixture(t)
  const content = Buffer.from('electron updater payload')
  const requestHeaders = []
  const executor = {
    createRequest(options, callback) {
      requestHeaders.push(options.headers)
      return {
        abort() {},
        end() {
          const body = Readable.from([content])
          body.statusCode = 200
          body.headers = { 'content-length': String(content.length) }
          callback(body)
        },
      }
    },
    addErrorAndTimeoutHandlers() {},
    addRedirectHandlers() {},
  }
  const updater = { httpExecutor: executor }
  assert.equal(enableResumableUpdateDownloads(updater), true)
  assert.equal(enableResumableUpdateDownloads(updater), false)

  await executor.download('https://example.test/Pisper.exe', paths.destination, {
    sha512: sha512(content),
    cancellationToken: { cancelled: false, onCancel() {}, removeListener() {} },
  })

  assert.equal(requestHeaders[0]['User-Agent'], 'Pisper Updater')
  assert.deepEqual(await readFile(paths.destination), content)
})
