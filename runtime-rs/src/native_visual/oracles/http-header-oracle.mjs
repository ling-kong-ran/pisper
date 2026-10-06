// Test-only execution of the read-only release drivers against a loopback HTTP
// server. Never read a profile, call a provider, or import this from production.
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { registerHooks } from 'node:module'
import { dirname, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { readFile, mkdtemp, writeFile, unlink, rmdir } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { tmpdir } from 'node:os'

const workspace = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..')
const reference = resolve(workspace, '../pisper-release-parity-reference')
const sdkUrl = pathToFileURL(resolve(workspace, 'node_modules/openai/index.mjs')).href
const hooks = registerHooks({ resolve(specifier, context, nextResolve) {
  if (specifier === 'openai') return { url: sdkUrl, shortCircuit: true }
  return nextResolve(specifier, context)
}})
const referenceDir = resolve(reference, 'runtime/services/visual-generation')
const { generateOpenAICompatible } = await import(pathToFileURL(resolve(referenceDir, 'openai-compatible.mjs')).href)
const { generateGoogle } = await import(pathToFileURL(resolve(referenceDir, 'google.mjs')).href)
const { generateXAI } = await import(pathToFileURL(resolve(referenceDir, 'xai.mjs')).href)
const { VisualModelCatalog } = await import(pathToFileURL(resolve(referenceDir, 'model-selection.mjs')).href)
const records = []
const png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgAAAAAgABVKJPXQAAAABJRU5ErkJggg=='
const server = createServer(async (request, response) => {
  const chunks = []
  for await (const chunk of request) chunks.push(chunk)
  records.push({ method: request.method, path: request.url, headers: request.headers, body: Buffer.concat(chunks).toString() })
  response.setHeader('content-type', 'application/json')
  if (request.url.endsWith('/content?variant=video')) {
    response.statusCode = 429
    response.setHeader('x-should-retry', 'false')
    response.end(JSON.stringify({ error: { message: 'download rate limited' } }))
  } else if (request.url.endsWith('/videos')) {
    response.end(JSON.stringify({ id: 'video-1', status: 'completed' }))
  } else if (request.url.includes(':generateContent')) {
    response.end(JSON.stringify({ candidates: [{ content: { parts: [{ inlineData: { mimeType: 'image/png', data: png } }] } }] }))
  } else response.end(JSON.stringify({ data: [{ b64_json: png }] }))
})
await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve) })
const baseUrl = `http://127.0.0.1:${server.address().port}/v1`
const base = { id: 'gpt-image-fixture', apiKey: 'synthetic-visual-key', baseUrl, api: 'openai-responses' }
const request = { kind: 'image', operation: 'generate', prompt: 'controlled fixture result', sourceImages: [] }
const cases = []
try {
  for (const [name, generate] of [['sdk', generateOpenAICompatible], ['google', generateGoogle], ['xai', generateXAI]]) {
    await generate({ ...base, driver: `${name}-image`, headers: { Authorization: null, 'X-Test': ['a', 'b'], 'Content-Type': null } }, request, { allowFallback: false })
    const record = records.at(-1)
    if (name === 'sdk') {
      assert.equal(record.headers.authorization, undefined)
      assert.equal(record.headers['x-test'], 'a, b')
      assert.equal(record.headers['content-type'], 'application/json')
    } else {
      assert.equal(record.headers.authorization, 'null')
      assert.equal(record.headers['x-test'], 'a,b')
      assert.equal(record.headers['content-type'], name === 'google' ? 'null' : 'application/json')
    }
    cases.push({ name, method: record.method, path: record.path, authorization: record.headers.authorization ?? null, test: record.headers['x-test'], contentType: record.headers['content-type'] })
  }
  await generateXAI({ ...base, headers: { authorization: 'override' } }, request, { allowFallback: false })
  assert.equal(records.at(-1).headers.authorization, 'Bearer synthetic-visual-key, override')
  cases.push({ name: 'fetch-case-spread', authorization: records.at(-1).headers.authorization })
  await generateOpenAICompatible({ ...base, headers: { 'Content-Type': 'ignored-custom' } }, { ...request, operation: 'edit', sourceImages: [{ path: 'fixture.png', buffer: Buffer.from(png, 'base64'), mimeType: 'image/png' }] }, { allowFallback: false })
  assert.equal(records.at(-1).headers['content-type'], 'ignored-custom')
  cases.push({ name: 'sdk-multipart', contentType: records.at(-1).headers['content-type'] })
  await assert.rejects(() => generateOpenAICompatible({ ...base, headers: {} }, { kind: 'video', operation: 'generate', prompt: request.prompt }, { allowFallback: false }), error => error.status === 429 && error.message.includes('download rate limited'))
  cases.push({ name: 'sdk-video-content-error', status: 429 })
  const temp = await mkdtemp(resolve(tmpdir(), 'pisper-visual-oracle-'))
  const paths = { modelsPath: resolve(temp, 'models.json'), authPath: resolve(temp, 'auth.json'), appConfigPath: resolve(temp, 'pisper.json') }
  const documents = { modelsPath: { providers: { fixture: { api: 'openai-responses', baseUrl, models: [{ id: 'gpt-image-fixture', kind: 'image' }] } } }, authPath: { fixture: 'synthetic-visual-key' }, appConfigPath: {} }
  try {
    for (const [name, expected] of [['appConfigPath', "Cannot read properties of null (reading 'disabledProviders')"], ['modelsPath', "Cannot read properties of null (reading 'providers')"], ['authPath', "Cannot read properties of null (reading 'fixture')"]]) {
      for (const [key, path] of Object.entries(paths)) await writeFile(path, JSON.stringify(key === name ? null : documents[key]))
      await assert.rejects(() => new VisualModelCatalog(paths).status('image'), error => error.message === expected)
      cases.push({ name: `raw-null-${name}`, error: expected })
    }
    for (const primitive of [false, 0, 'raw document', []]) {
      for (const [key, path] of Object.entries(paths)) await writeFile(path, JSON.stringify(key === 'appConfigPath' ? primitive : documents[key]))
      const status = await new VisualModelCatalog(paths).status('image')
      assert.equal(status.model.id, 'gpt-image-fixture')
      cases.push({ name: 'raw-primitive-app', value: primitive, model: status.model.id })
    }
  } finally {
    for (const path of Object.values(paths)) await unlink(path)
    await rmdir(temp)
  }
  const sdkPackage = JSON.parse(await readFile(resolve(workspace, 'node_modules/openai/package.json'), 'utf8'))
  const sources = {}
  for (const name of ['openai-compatible.mjs', 'google.mjs', 'xai.mjs']) sources[name] = createHash('sha256').update(await readFile(resolve(referenceDir, name))).digest('hex')
  process.stdout.write(JSON.stringify({ passed: cases.length, failed: 0, paidRequests: 0, personalProfileRead: false, oracleCommit: '582160235671903d9f1c7034b457557b1df74b68', sdkVersion: sdkPackage.version, sources, cases }, null, 2) + '\n')
} finally {
  await new Promise(resolve => server.close(resolve))
  hooks.deregister()
}
