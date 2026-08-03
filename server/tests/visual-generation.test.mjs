import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import test from 'node:test'
import { VisualGenerationService } from '../services/visual-generation/index.mjs'
import { VisualModelCatalog } from '../services/visual-generation/model-selection.mjs'
import { TOOL_CATALOG, createAppTools } from '../tools/registry.mjs'

const PNG = Buffer.from(
  '89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082',
  'hex',
)
const MP4 = Buffer.from('00000018667479706d703432000000006d70343269736f6d', 'hex')

async function listen(handler) {
  const server = createServer(handler)
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  return { server, port: server.address().port }
}

async function fixture(provider) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-visual-test-'))
  const modelsPath = join(directory, 'models.json')
  const authPath = join(directory, 'auth.json')
  const appConfigPath = join(directory, 'pisper.json')
  await writeFile(modelsPath, JSON.stringify({ providers: { [provider.id]: provider.config } }))
  await writeFile(authPath, JSON.stringify({ [provider.id]: { type: 'api_key', key: 'test-key' } }))
  await writeFile(appConfigPath, JSON.stringify({ disabledProviders: [] }))
  return {
    directory,
    service: new VisualGenerationService({ modelsPath, authPath, appConfigPath }),
    cleanup: () => rm(directory, { recursive: true, force: true }),
  }
}

test('visual model catalog deduplicates shared relay models and prefers the explicit visual Provider', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-visual-catalog-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const sharedBaseUrl = 'https://relay.example.test/v1'
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        openai: { name: 'Chat Provider', api: 'openai-responses', baseUrl: sharedBaseUrl },
        'openai-image': {
          name: 'Visual Provider',
          api: 'openai-responses',
          baseUrl: sharedBaseUrl,
          models: [{ id: 'gpt-image-2', name: 'A primary image', kind: 'image' }],
        },
        'visual-backup': {
          name: 'Visual Backup',
          api: 'openai-responses',
          baseUrl: 'https://backup.example.test/v1',
          models: [{ id: 'gpt-image-2', name: 'Z backup image', kind: 'image' }],
        },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({
      openai: { type: 'api_key', key: 'chat-key' },
      'openai-image': { type: 'api_key', key: 'visual-key' },
      'visual-backup': { type: 'api_key', key: 'backup-key' },
    }),
  )
  await writeFile(
    join(directory, 'pisper.json'),
    JSON.stringify({
      disabledProviders: [],
      providerTypes: { openai: 'chat', 'openai-image': 'visual', 'visual-backup': 'visual' },
    }),
  )
  const runtimeModels = {
    openai: [
      {
        id: 'gpt-image-2',
        name: 'Discovered image copy',
        api: 'openai-responses',
        baseUrl: sharedBaseUrl,
        pisperKind: 'image',
      },
    ],
  }
  const runtime = {
    getProviders: () => [
      { id: 'openai', name: 'OpenAI' },
      { id: 'openai-image', name: 'Visual Provider' },
      { id: 'visual-backup', name: 'Visual Backup' },
    ],
    getProvider: (id) => ({ id, name: id }),
    getModels: (id) => runtimeModels[id] || [],
  }
  const catalog = new VisualModelCatalog({
    modelsPath: join(directory, 'models.json'),
    authPath: join(directory, 'auth.json'),
    appConfigPath: join(directory, 'pisper.json'),
    getModelRuntime: () => runtime,
  })

  const models = await catalog.list('image')
  assert.equal(models.length, 2)
  assert.ok(
    models.some((model) => model.providerId === 'openai-image' && model.baseUrl === sharedBaseUrl),
  )
  assert.ok(models.some((model) => model.providerId === 'visual-backup'))
  assert.ok(!models.some((model) => model.providerId === 'openai'))
  const automatic = await catalog.select('image')
  const unqualified = await catalog.select('image', 'gpt-image-2')
  const visualQualified = await catalog.select('image', 'openai-image/gpt-image-2')
  const chatQualified = await catalog.select('image', 'openai/gpt-image-2')
  assert.equal(automatic.providerId, 'openai-image')
  assert.equal(automatic.apiKey, 'visual-key')
  assert.equal(unqualified.providerId, 'openai-image')
  assert.equal(unqualified.apiKey, 'visual-key')
  assert.equal(visualQualified.providerId, 'openai-image')
  assert.equal(visualQualified.apiKey, 'visual-key')
  assert.equal(chatQualified.providerId, 'openai')
  assert.equal(chatQualified.apiKey, 'chat-key')
})

test('automatic visual ordering prefers backup models from a dedicated visual Provider', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-visual-order-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const sharedBaseUrl = 'https://relay.example.test/v1'
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        'openai-image': {
          name: 'Dedicated Visual',
          api: 'openai-responses',
          baseUrl: sharedBaseUrl,
          models: [{ id: 'gpt-image-2', kind: 'image' }],
        },
        xai: { name: 'Chat and Grok', api: 'openai-completions', baseUrl: sharedBaseUrl },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({
      'openai-image': { type: 'api_key', key: 'visual-key' },
      xai: { type: 'api_key', key: 'xai-key' },
    }),
  )
  await writeFile(
    join(directory, 'pisper.json'),
    JSON.stringify({
      disabledProviders: [],
      providerTypes: { 'openai-image': 'visual', xai: 'chat' },
    }),
  )
  const runtimeModels = {
    'openai-image': [{ id: 'grok-imagine-image', pisperKind: 'image', baseUrl: sharedBaseUrl }],
    xai: [{ id: 'grok-imagine-image-quality', pisperKind: 'image', baseUrl: sharedBaseUrl }],
  }
  const catalog = new VisualModelCatalog({
    modelsPath: join(directory, 'models.json'),
    authPath: join(directory, 'auth.json'),
    appConfigPath: join(directory, 'pisper.json'),
    getModelRuntime: () => ({
      getProviders: () => [{ id: 'openai-image' }, { id: 'xai' }],
      getProvider: (id) => ({ id, name: id }),
      getModels: (id) => runtimeModels[id] || [],
    }),
  })

  const models = await catalog.list('image')
  assert.deepEqual(
    models.map((model) => `${model.providerId}/${model.id}`),
    [
      'openai-image/gpt-image-2',
      'openai-image/grok-imagine-image',
      'xai/grok-imagine-image-quality',
    ],
  )
})

test('a duplicated chat catalog image still sends the dedicated visual Provider credential', async (t) => {
  let authorization = ''
  let requestedModel = ''
  const { server, port } = await listen(async (req, res) => {
    if (req.method === 'POST' && req.url === '/v1/images/generations') {
      authorization = String(req.headers.authorization || '')
      const chunks = []
      for await (const chunk of req) chunks.push(chunk)
      requestedModel = JSON.parse(Buffer.concat(chunks).toString('utf8')).model
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ data: [{ b64_json: PNG.toString('base64') }] }))
      return
    }
    res.writeHead(404).end()
  })
  t.after(() => server.close())
  const directory = await mkdtemp(join(tmpdir(), 'pisper-visual-credential-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const baseUrl = `http://127.0.0.1:${port}/v1`
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        openai: { name: 'Chat Provider', api: 'openai-responses', baseUrl },
        'openai-image': {
          name: 'Visual Provider',
          api: 'openai-responses',
          baseUrl,
          models: [{ id: 'gpt-image-2', kind: 'image' }],
        },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({
      openai: { type: 'api_key', key: 'chat-key' },
      'openai-image': { type: 'api_key', key: 'visual-key' },
    }),
  )
  await writeFile(
    join(directory, 'pisper.json'),
    JSON.stringify({
      disabledProviders: [],
      providerTypes: { openai: 'chat', 'openai-image': 'visual' },
    }),
  )
  const runtime = {
    getProviders: () => [{ id: 'openai' }, { id: 'openai-image' }],
    getProvider: (id) => ({ id, name: id }),
    getModels: (id) =>
      id === 'openai'
        ? [{ id: 'gpt-image-2', api: 'openai-responses', baseUrl, pisperKind: 'image' }]
        : [],
  }
  const service = new VisualGenerationService({
    modelsPath: join(directory, 'models.json'),
    authPath: join(directory, 'auth.json'),
    appConfigPath: join(directory, 'pisper.json'),
    getModelRuntime: () => runtime,
  })

  const result = await service.generate({
    kind: 'image',
    prompt: 'credential routing test',
    cwd: directory,
  })
  assert.equal(result.provider, 'openai-image')
  assert.equal(authorization, 'Bearer visual-key')
  assert.equal(requestedModel, 'gpt-image-2')
})

test('automatic visual selection falls back when the preferred model has no available channel', async () => {
  const requestedModels = []
  const progress = []
  const { server, port } = await listen(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1')
    if (req.method === 'GET' && url.pathname === '/') {
      res.writeHead(404).end()
      return
    }
    if (req.method === 'POST' && url.pathname === '/v1/images/generations') {
      const chunks = []
      for await (const chunk of req) chunks.push(chunk)
      const model = JSON.parse(Buffer.concat(chunks).toString('utf8')).model
      requestedModels.push(model)
      if (model === 'gpt-image-2') {
        res.writeHead(503, { 'content-type': 'application/json' })
        res.end(
          JSON.stringify({
            error: {
              code: 'model_not_found',
              message: '分组 image 下模型 gpt-image-2 无可用渠道（distributor）',
            },
          }),
        )
        return
      }
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ data: [{ b64_json: PNG.toString('base64') }] }))
      return
    }
    res.writeHead(404).end()
  })
  const value = await fixture({
    id: 'fallback-relay',
    config: {
      name: 'Fallback Relay',
      api: 'openai-responses',
      baseUrl: `http://127.0.0.1:${port}/v1`,
      models: [
        { id: 'gpt-image-2', kind: 'image' },
        { id: 'grok-imagine-image', kind: 'image' },
      ],
    },
  })
  try {
    const result = await value.service.generate(
      { kind: 'image', prompt: 'automatic fallback test', cwd: value.directory },
      { onProgress: (message) => progress.push(message) },
    )
    assert.equal(result.model, 'grok-imagine-image')
    assert.equal(result.fallbackUsed, true)
    assert.deepEqual(result.attemptedModels, [
      'fallback-relay/gpt-image-2',
      'fallback-relay/grok-imagine-image',
    ])
    assert.equal(requestedModels[0], 'gpt-image-2')
    assert.equal(requestedModels.at(-1), 'grok-imagine-image')
    assert.ok(
      progress.some(
        (message) => message.includes('当前不可用') && message.includes('grok-imagine-image'),
      ),
    )
  } finally {
    server.close()
    await value.cleanup()
  }
})

test('automatic visual selection falls back when the token cannot access only the preferred model', async () => {
  const requestedModels = []
  const { server, port } = await listen(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1')
    if (req.method === 'GET' && url.pathname === '/') {
      res.writeHead(404).end()
      return
    }
    if (req.method === 'POST' && url.pathname === '/v1/images/generations') {
      const chunks = []
      for await (const chunk of req) chunks.push(chunk)
      const model = JSON.parse(Buffer.concat(chunks).toString('utf8')).model
      requestedModels.push(model)
      if (model === 'gpt-image-2') {
        res.writeHead(403, { 'content-type': 'application/json' })
        res.end(JSON.stringify({ error: { message: '该令牌无权访问模型 gpt-image-2' } }))
        return
      }
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ data: [{ b64_json: PNG.toString('base64') }] }))
      return
    }
    res.writeHead(404).end()
  })
  const value = await fixture({
    id: 'model-permission-relay',
    config: {
      name: 'Model Permission Relay',
      api: 'openai-responses',
      baseUrl: `http://127.0.0.1:${port}/v1`,
      models: [
        { id: 'gpt-image-2', kind: 'image' },
        { id: 'grok-imagine-image', kind: 'image' },
      ],
    },
  })
  try {
    const result = await value.service.generate({
      kind: 'image',
      prompt: 'model permission fallback test',
      cwd: value.directory,
    })
    assert.equal(result.model, 'grok-imagine-image')
    assert.equal(result.fallbackUsed, true)
    assert.equal(requestedModels[0], 'gpt-image-2')
    assert.equal(requestedModels.at(-1), 'grok-imagine-image')
  } finally {
    server.close()
    await value.cleanup()
  }
})

test('automatic visual selection does not use another model to bypass a safety rejection', async () => {
  const requestedModels = []
  const { server, port } = await listen(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1')
    if (req.method === 'POST' && url.pathname === '/v1/images/generations') {
      const chunks = []
      for await (const chunk of req) chunks.push(chunk)
      requestedModels.push(JSON.parse(Buffer.concat(chunks).toString('utf8')).model)
      res.writeHead(403, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ error: { message: 'Content policy safety rejection' } }))
      return
    }
    res.writeHead(404).end()
  })
  const value = await fixture({
    id: 'safety-relay',
    config: {
      name: 'Safety Relay',
      api: 'openai-responses',
      baseUrl: `http://127.0.0.1:${port}/v1`,
      models: [
        { id: 'gpt-image-2', kind: 'image' },
        { id: 'grok-imagine-image', kind: 'image' },
      ],
    },
  })
  try {
    await assert.rejects(
      value.service.generate({
        kind: 'image',
        prompt: 'safety rejection test',
        cwd: value.directory,
      }),
      /Content policy safety rejection/,
    )
    assert.deepEqual(requestedModels, ['gpt-image-2'])
  } finally {
    server.close()
    await value.cleanup()
  }
})

test('an explicitly requested visual model fails without switching to another model', async () => {
  const requestedModels = []
  const { server, port } = await listen(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1')
    if (req.method === 'POST' && url.pathname === '/v1/images/generations') {
      const chunks = []
      for await (const chunk of req) chunks.push(chunk)
      requestedModels.push(JSON.parse(Buffer.concat(chunks).toString('utf8')).model)
      res.writeHead(503, { 'content-type': 'application/json' })
      res.end(
        JSON.stringify({
          error: { code: 'model_not_found', message: 'gpt-image-2 has no available channel' },
        }),
      )
      return
    }
    res.writeHead(404).end()
  })
  const value = await fixture({
    id: 'explicit-relay',
    config: {
      name: 'Explicit Relay',
      api: 'openai-responses',
      baseUrl: `http://127.0.0.1:${port}/v1`,
      models: [
        { id: 'gpt-image-2', kind: 'image' },
        { id: 'grok-imagine-image', kind: 'image' },
      ],
    },
  })
  try {
    await assert.rejects(
      value.service.generate({
        kind: 'image',
        prompt: 'explicit model test',
        model: 'gpt-image-2',
        cwd: value.directory,
      }),
      /no available channel/,
    )
    assert.ok(requestedModels.length >= 1)
    assert.ok(requestedModels.every((model) => model === 'gpt-image-2'))
  } finally {
    server.close()
    await value.cleanup()
  }
})

test('OpenAI-compatible image and video models generate files', async () => {
  const { server, port } = await listen(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1')
    if (req.method === 'POST' && url.pathname === '/v1/images/generations') {
      for await (const _chunk of req) void _chunk
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ created: Date.now(), data: [{ b64_json: PNG.toString('base64') }] }))
      return
    }
    if (req.method === 'POST' && url.pathname === '/v1/images/edits') {
      for await (const _chunk of req) void _chunk
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ created: Date.now(), data: [{ b64_json: PNG.toString('base64') }] }))
      return
    }
    if (req.method === 'POST' && url.pathname === '/v1/videos') {
      for await (const _chunk of req) void _chunk
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(
        JSON.stringify({
          id: 'video-test',
          object: 'video',
          status: 'completed',
          progress: 100,
          model: 'sora-2',
          prompt: 'test',
          seconds: '4',
          size: '1280x720',
          created_at: 1,
          completed_at: 2,
          expires_at: null,
          error: null,
          remixed_from_video_id: null,
        }),
      )
      return
    }
    if (req.method === 'GET' && url.pathname === '/v1/videos/video-test/content') {
      res.writeHead(200, { 'content-type': 'video/mp4' })
      res.end(MP4)
      return
    }
    res.writeHead(404).end()
  })
  const value = await fixture({
    id: 'fake',
    config: {
      name: 'Fake Visual',
      api: 'openai-responses',
      baseUrl: `http://127.0.0.1:${port}/v1`,
      models: [
        { id: 'gpt-image-1', kind: 'image' },
        { id: 'sora-2', kind: 'video' },
      ],
    },
  })
  try {
    const sourcePath = join(value.directory, 'source.png')
    await writeFile(sourcePath, PNG)
    const image = await value.service.generate({
      kind: 'image',
      prompt: 'test image',
      cwd: value.directory,
    })
    const edited = await value.service.generate({
      kind: 'image',
      prompt: 'make the background blue',
      sourceImages: [sourcePath],
      cwd: value.directory,
    })
    const video = await value.service.generate({
      kind: 'video',
      prompt: 'test video',
      cwd: value.directory,
      durationSeconds: 4,
      size: '1280x720',
    })
    assert.equal((await readFile(image.path)).length, PNG.length)
    assert.equal((await readFile(edited.path)).length, PNG.length)
    assert.equal(edited.operation, 'edit')
    assert.equal((await readFile(video.path)).length, MP4.length)
    assert.equal(video.mimeType, 'video/mp4')
    assert.ok(TOOL_CATALOG.some((tool) => tool.id === 'generate_visual'))
    assert.ok(!TOOL_CATALOG.some((tool) => tool.id === 'workspace_summary'))
    let indexedPath = ''
    const tools = createAppTools({
      cwd: value.directory,
      enabledTools: ['generate_visual'],
      visualGenerationService: value.service,
      onGeneratedFile: (result) => {
        indexedPath = result.path
      },
    })
    assert.equal(tools[0].name, 'generate_visual')
    await tools[0].execute(
      'tool-test',
      { kind: 'image', prompt: 'tool image', model: 'gpt-image-1' },
      new AbortController().signal,
    )
    assert.ok(indexedPath.endsWith('.png'))
  } finally {
    server.close()
    await value.cleanup()
  }
})

test('Google Gemini and Veo models generate files', async () => {
  let port
  const { server, port: listeningPort } = await listen(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1')
    if (req.method === 'POST' && url.pathname.endsWith(':generateContent')) {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(
        JSON.stringify({
          candidates: [
            {
              content: {
                parts: [{ inlineData: { mimeType: 'image/png', data: PNG.toString('base64') } }],
              },
            },
          ],
        }),
      )
      return
    }
    if (req.method === 'POST' && url.pathname.endsWith(':predictLongRunning')) {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(
        JSON.stringify({
          name: 'operations/video-test',
          done: true,
          response: { generatedVideos: [{ video: { uri: `http://127.0.0.1:${port}/video.mp4` } }] },
        }),
      )
      return
    }
    if (req.method === 'GET' && url.pathname === '/video.mp4') {
      res.writeHead(200, { 'content-type': 'video/mp4' })
      res.end(MP4)
      return
    }
    res.writeHead(404).end()
  })
  port = listeningPort
  const value = await fixture({
    id: 'google',
    config: {
      name: 'Google',
      api: 'google-generative-ai',
      baseUrl: `http://127.0.0.1:${port}/v1beta`,
      models: [
        { id: 'gemini-3-pro-image', kind: 'image' },
        { id: 'veo-3.1-generate-preview', kind: 'video' },
      ],
    },
  })
  try {
    const image = await value.service.generate({
      kind: 'image',
      prompt: 'test image',
      cwd: value.directory,
      aspectRatio: '16:9',
    })
    const video = await value.service.generate({
      kind: 'video',
      prompt: 'test video',
      cwd: value.directory,
      aspectRatio: '16:9',
    })
    assert.equal((await readFile(image.path)).length, PNG.length)
    assert.equal((await readFile(video.path)).length, MP4.length)
  } finally {
    server.close()
    await value.cleanup()
  }
})

test('Grok visual models use xAI image and video endpoints', async () => {
  const requests = []
  let port
  const { server, port: listeningPort } = await listen(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1')
    requests.push(`${req.method} ${url.pathname}`)
    if (req.method === 'POST' && url.pathname === '/v1/images/generations') {
      for await (const _chunk of req) void _chunk
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ data: [{ b64_json: PNG.toString('base64') }] }))
      return
    }
    if (req.method === 'POST' && url.pathname === '/v1/images/edits') {
      for await (const _chunk of req) void _chunk
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ data: [{ b64_json: PNG.toString('base64') }] }))
      return
    }
    if (req.method === 'POST' && url.pathname === '/v1/videos/generations') {
      for await (const _chunk of req) void _chunk
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(
        JSON.stringify({
          task_id: 'grok-video',
          status: 'succeeded',
          url: `http://127.0.0.1:${port}/grok.mp4`,
        }),
      )
      return
    }
    if (req.method === 'GET' && url.pathname === '/grok.mp4') {
      res.writeHead(200, { 'content-type': 'video/mp4' })
      res.end(MP4)
      return
    }
    res.writeHead(404).end()
  })
  port = listeningPort
  const value = await fixture({
    id: 'grok-relay',
    config: {
      name: 'Grok Relay',
      api: 'openai-responses',
      baseUrl: `http://127.0.0.1:${port}/v1`,
      models: [
        { id: 'grok-imagine-image', kind: 'image' },
        { id: 'grok-imagine-video', kind: 'video' },
      ],
    },
  })
  try {
    const sourcePath = join(value.directory, 'grok-source.png')
    await writeFile(sourcePath, PNG)
    const image = await value.service.generate({
      kind: 'image',
      prompt: 'test Grok image',
      cwd: value.directory,
    })
    const edited = await value.service.generate({
      kind: 'image',
      prompt: 'edit Grok image',
      sourceImages: [sourcePath],
      cwd: value.directory,
    })
    const video = await value.service.generate({
      kind: 'video',
      prompt: 'test Grok video',
      cwd: value.directory,
      durationSeconds: 4,
    })
    assert.equal((await readFile(image.path)).length, PNG.length)
    assert.equal((await readFile(edited.path)).length, PNG.length)
    assert.equal((await readFile(video.path)).length, MP4.length)
    assert.ok(requests.includes('POST /v1/images/edits'))
    assert.ok(requests.includes('POST /v1/videos/generations'))
    assert.equal(video.mimeType, 'video/mp4')
  } finally {
    server.close()
    await value.cleanup()
  }
})

test('New API relays are detected automatically and use their video task protocol', async () => {
  const requests = []
  const { server, port } = await listen(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1')
    requests.push(`${req.method} ${url.pathname}`)
    if (req.method === 'GET' && url.pathname === '/') {
      res.writeHead(200, { 'content-type': 'text/html' })
      res.end(
        '<title>New API</title><meta name="description" content="Unified AI API gateway and admin dashboard.">',
      )
      return
    }
    if (req.method === 'POST' && url.pathname === '/v1/videos') {
      for await (const _chunk of req) void _chunk
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ id: 'new-api-video', object: 'video', status: 'completed' }))
      return
    }
    if (req.method === 'GET' && url.pathname === '/v1/videos/new-api-video/content') {
      res.writeHead(200, { 'content-type': 'video/mp4' })
      res.end(MP4)
      return
    }
    res.writeHead(404).end()
  })
  const value = await fixture({
    id: 'new-api-relay',
    config: {
      name: 'New API Relay',
      api: 'openai-responses',
      baseUrl: `http://127.0.0.1:${port}/v1`,
      models: [{ id: 'grok-imagine-video', kind: 'video' }],
    },
  })
  try {
    const video = await value.service.generate({
      kind: 'video',
      prompt: 'test New API video',
      cwd: value.directory,
      durationSeconds: 4,
      aspectRatio: '16:9',
    })
    assert.equal((await readFile(video.path)).length, MP4.length)
    assert.equal(video.mimeType, 'video/mp4')
    assert.ok(requests.includes('GET /'))
    assert.ok(requests.includes('POST /v1/videos'))
    assert.ok(requests.includes('GET /v1/videos/new-api-video/content'))
    assert.ok(!requests.includes('POST /v1/videos/generations'))
  } finally {
    server.close()
    await value.cleanup()
  }
})

test('New API channel conversion failures are exposed without retrying another video route', async () => {
  const requests = []
  const { server, port } = await listen(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1')
    requests.push(`${req.method} ${url.pathname}`)
    if (req.method === 'GET' && url.pathname === '/') {
      res.writeHead(200, { 'content-type': 'text/html' })
      res.end('<title>New API</title>')
      return
    }
    if (req.method === 'POST' && url.pathname === '/v1/videos') {
      for await (const _chunk of req) void _chunk
      res.writeHead(400, { 'content-type': 'application/json' })
      res.end(
        JSON.stringify({
          message:
            'Failed to deserialize the JSON body into the target type: duplicate field `duration`',
        }),
      )
      return
    }
    res.writeHead(404).end()
  })
  const value = await fixture({
    id: 'new-api-broken-relay',
    config: {
      name: 'New API Relay',
      api: 'openai-responses',
      baseUrl: `http://127.0.0.1:${port}/v1`,
      models: [{ id: 'grok-imagine-video', kind: 'video' }],
    },
  })
  try {
    await assert.rejects(
      value.service.generate({
        kind: 'video',
        prompt: 'test broken relay',
        cwd: value.directory,
        durationSeconds: 4,
      }),
      /New API 视频渠道转发失败.*duplicate field `duration`.*渠道映射或协议适配/,
    )
    assert.deepEqual(requests, ['GET /', 'POST /v1/videos'])
  } finally {
    server.close()
    await value.cleanup()
  }
})
