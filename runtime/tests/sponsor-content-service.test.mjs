import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import {
  SponsorContentService,
  validateSponsorDocument,
} from '../services/sponsor-content-service.mjs'

function sponsorDocument(description = 'Remote Matrix sponsor') {
  return {
    schemaVersion: 1,
    campaigns: [
      {
        id: 'matrix',
        placement: 'settings-updates',
        enabled: true,
        priority: 100,
        name: { 'zh-CN': 'Matrix', 'en-US': 'Matrix' },
        description: { 'zh-CN': '感谢 Matrix 对 Pisper 社区的支持。', 'en-US': description },
        href: 'https://matrix.000328.xyz/sign-up?aff=ZPeH',
      },
    ],
  }
}

function request(method) {
  return { method }
}

function response() {
  return {
    status: 0,
    body: '',
    writeHead(status) {
      this.status = status
    },
    end(body = '') {
      this.body += body
    },
  }
}

test('bundled sponsor configuration preserves the Matrix referral link', async () => {
  const bundled = JSON.parse(
    await readFile(new URL('../../docs/sponsors.json', import.meta.url), 'utf8'),
  )
  const validated = validateSponsorDocument(bundled)
  assert.equal(validated.campaigns[0].name['zh-CN'], 'Matrix')
  assert.equal(validated.campaigns[0].href, 'https://matrix.000328.xyz/sign-up?aff=ZPeH')
})

test('sponsor content refreshes with ETag and survives offline restarts', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-sponsors-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  const fallbackPath = join(dataDir, 'bundled-sponsors.json')
  await writeFile(fallbackPath, `${JSON.stringify(sponsorDocument('Bundled sponsor'))}\n`)

  let now = Date.parse('2026-07-30T00:00:00.000Z')
  const requests = []
  const service = new SponsorContentService({
    dataDir,
    fallbackPath,
    appVersion: '0.4.3',
    now: () => now,
    fetcher: async (url, options) => {
      requests.push({ url, headers: options.headers })
      if (requests.length === 1) {
        return new Response(JSON.stringify(sponsorDocument()), {
          headers: { etag: '"matrix-v1"', 'content-type': 'application/json' },
        })
      }
      return new Response(null, { status: 304 })
    },
  })

  const first = await service.getPlacement('settings-updates', { locale: 'en-US' })
  const cached = await service.getPlacement('settings-updates', { locale: 'en-US' })
  assert.equal(first.campaigns[0].name, 'Matrix')
  assert.equal(first.campaigns[0].description, 'Remote Matrix sponsor')
  assert.equal(first.campaigns[0].href, 'https://matrix.000328.xyz/sign-up?aff=ZPeH')
  assert.deepEqual(cached, first)
  assert.equal(requests.length, 1)
  assert.match(requests[0].url, /contents\/docs\/sponsors\.json\?ref=main$/)
  assert.equal(requests[0].headers['User-Agent'], 'Pisper/0.4.3')

  now += 16 * 60_000
  await service.getPlacement('settings-updates', { locale: 'zh-CN' })
  assert.equal(requests.length, 2)
  assert.equal(requests[1].headers['If-None-Match'], '"matrix-v1"')

  const persisted = JSON.parse(await readFile(join(dataDir, 'sponsors-cache.json'), 'utf8'))
  assert.equal(persisted.etag, '"matrix-v1"')
  assert.equal(persisted.document.campaigns[0].id, 'matrix')

  now += 16 * 60_000
  const offline = new SponsorContentService({
    dataDir,
    fallbackPath,
    now: () => now,
    fetcher: async () => {
      throw new Error('offline')
    },
  })
  const restored = await offline.getPlacement('settings-updates', { locale: 'en-US' })
  assert.equal(restored.source, 'cache')
  assert.equal(restored.campaigns[0].description, 'Remote Matrix sponsor')
})

test('invalid remote content keeps the bundled sponsor fallback', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-sponsors-fallback-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  const fallbackPath = join(dataDir, 'bundled-sponsors.json')
  await writeFile(fallbackPath, `${JSON.stringify(sponsorDocument('Bundled sponsor'))}\n`)
  const service = new SponsorContentService({
    dataDir,
    fallbackPath,
    fetcher: async () =>
      new Response(JSON.stringify({ schemaVersion: 1, campaigns: [{ html: '<script />' }] })),
  })

  const result = await service.getPlacement('settings-updates', { locale: 'en-US' })
  assert.equal(result.source, 'bundled')
  assert.equal(result.campaigns[0].description, 'Bundled sponsor')
})

test('sponsor validation rejects executable and insecure remote content', () => {
  assert.throws(
    () =>
      validateSponsorDocument({
        ...sponsorDocument(),
        campaigns: [{ ...sponsorDocument().campaigns[0], href: 'javascript:alert(1)' }],
      }),
    /HTTPS/,
  )
  assert.throws(
    () =>
      validateSponsorDocument({
        ...sponsorDocument(),
        campaigns: [sponsorDocument().campaigns[0], { ...sponsorDocument().campaigns[0] }],
      }),
    /重复/,
  )
})

test('sponsor API delegates placement and locale without exposing remote fetching to the UI', async () => {
  const calls = []
  const sponsors = {
    async getPlacement(placement, options) {
      calls.push([placement, options])
      return { placement, campaigns: sponsorDocument().campaigns }
    },
  }
  const handler = createApiHandler({}, { sponsors })
  const output = response()
  const url = new URL('http://localhost/api/sponsors/settings-updates?locale=en-US&refresh=1')

  assert.equal(await handler(request('GET'), output, url), true)
  assert.equal(output.status, 200)
  assert.deepEqual(calls, [['settings-updates', { locale: 'en-US', refresh: true }]])
})
