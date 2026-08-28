import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

const PNG = Buffer.from(
  '89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082',
  'hex',
)

async function listen(handler) {
  const server = createServer(handler)
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  return { server, port: server.address().port }
}

async function createRuntime(t, directory) {
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  return runtime
}

async function writeVisualProvider(directory, baseUrl) {
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        'openai-image': {
          name: 'Visual Provider',
          api: 'openai-responses',
          baseUrl,
          models: [{ id: 'gpt-image-2', name: 'GPT Image 2', kind: 'image' }],
        },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({ 'openai-image': { type: 'api_key', key: 'visual-key' } }),
  )
  await writeFile(
    join(directory, 'pisper.json'),
    JSON.stringify({ disabledProviders: [], providerTypes: { 'openai-image': 'visual' } }),
  )
}

test('visual model status reports the auto-selected image model and empty video slot', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-visual-status-'))
  const runtime = await createRuntime(t, directory)
  await writeVisualProvider(directory, 'https://visual.example.test/v1')

  const status = await runtime.getVisualModelStatus()
  assert.equal(status.image?.id, 'gpt-image-2')
  assert.equal(status.image?.providerId, 'openai-image')
  assert.equal(status.video, null)
})

test('visual model status is empty when no visual model is configured', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-visual-status-empty-'))
  const runtime = await createRuntime(t, directory)

  const status = await runtime.getVisualModelStatus()
  assert.equal(status.image, null)
  assert.equal(status.video, null)
})

test('visual generation smoke test writes output under the data directory and returns a preview', async (t) => {
  const { server, port } = await listen((req, res) => {
    if (req.method === 'POST' && req.url === '/v1/images/generations') {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ data: [{ b64_json: PNG.toString('base64') }] }))
      return
    }
    res.writeHead(404).end()
  })
  t.after(() => server.close())

  const directory = await mkdtemp(join(tmpdir(), 'pisper-visual-smoke-'))
  const runtime = await createRuntime(t, directory)
  await writeVisualProvider(directory, `http://127.0.0.1:${port}/v1`)

  const result = await runtime.testVisualGeneration()
  assert.ok(result.path.startsWith(join(directory, 'visual-test')))
  assert.equal(result.providerName, 'Visual Provider')
  assert.ok(result.previewDataUrl.startsWith('data:image/'))
})
