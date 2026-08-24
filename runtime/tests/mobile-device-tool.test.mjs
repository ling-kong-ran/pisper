import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { ToolPluginService } from '../services/tool-plugin-service.mjs'
import { TOOL_CATALOG, TOOL_PRESETS, createAppTools } from '../tools/registry.mjs'

test('mobile_device 是默认启用的高风险 App 工具', () => {
  const catalog = TOOL_CATALOG.find((tool) => tool.id === 'mobile_device')
  assert.equal(catalog?.risk, 'high')
  assert.equal(catalog?.source, 'app')
  assert.equal(TOOL_PRESETS.full.includes('mobile_device'), true)
  assert.equal(TOOL_PRESETS.workspace.includes('mobile_device'), false)
})

test('mobile_device V2 默认迁移会修复旧版遗漏的工具开关', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mobile-device-default-'))
  const configPath = join(directory, 'pisper.json')
  const service = new ToolPluginService(configPath)
  try {
    await writeFile(
      configPath,
      JSON.stringify({
        toolMode: 'custom',
        enabledTools: ['read'],
        mobileDeviceToolV1: true,
      }),
      'utf8',
    )
    await service.ensureDefaultTools(['mobile_device'], 'mobileDeviceToolV2')
    const config = JSON.parse(await readFile(configPath, 'utf8'))
    assert.equal(config.mobileDeviceToolV2, true)
    assert.deepEqual(config.enabledTools, ['read', 'mobile_device'])
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})

test('mobile_device 绑定当前会话并把相机结果写入私有捕获目录', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mobile-device-'))
  const calls = []
  const generated = []
  const [tool] = createAppTools({
    enabledTools: ['mobile_device'],
    mobileSessionId: 'session-mobile',
    mobileCaptureDir: directory,
    mobileOperationService: {
      async execute(sessionId, operation, parameters) {
        calls.push({ sessionId, operation, parameters })
        if (operation === 'camera.capture') {
          return {
            data: Buffer.from('jpeg-data').toString('base64'),
            width: 320,
            height: 240,
          }
        }
        return { contacts: [{ id: '1', name: 'Ada', phones: ['10086'] }] }
      },
    },
    onGeneratedFile: (asset) => generated.push(asset),
  })

  try {
    const contacts = await tool.execute('call-1', {
      action: 'search_contacts',
      query: 'Ada',
      limit: 5,
    })
    assert.match(contacts.content[0].text, /Ada/)
    assert.deepEqual(calls[0], {
      sessionId: 'session-mobile',
      operation: 'contacts.search',
      parameters: { query: 'Ada', limit: 5 },
    })

    const photo = await tool.execute('call-2', {
      action: 'capture_photo',
      cameraDirection: 'back',
    })
    assert.equal((await readFile(photo.details.path)).toString(), 'jpeg-data')
    assert.equal(generated[0].path, photo.details.path)
    assert.equal(photo.details.width, 320)
    assert.equal(photo.content[1].type, 'image')
    assert.equal(photo.content[1].mimeType, 'image/jpeg')

    await tool.execute('call-3', {
      action: 'open_url',
      url: 'https://example.com/docs?q=pisper',
    })
    assert.deepEqual(calls[2], {
      sessionId: 'session-mobile',
      operation: 'apps.open_url',
      parameters: { url: 'https://example.com/docs?q=pisper' },
    })

    await tool.execute('call-4', {
      action: 'compose_sms',
      phoneNumber: '+86 138-0013-8000',
      message: '请确认后手动发送',
    })
    assert.deepEqual(calls[3], {
      sessionId: 'session-mobile',
      operation: 'apps.compose_sms',
      parameters: {
        phoneNumber: '+86 138-0013-8000',
        message: '请确认后手动发送',
      },
    })

    await tool.execute('call-5', {
      action: 'open_app',
      packageName: 'com.tencent.mm',
    })
    assert.deepEqual(calls[4], {
      sessionId: 'session-mobile',
      operation: 'apps.open_app',
      parameters: { packageName: 'com.tencent.mm' },
    })

    await assert.rejects(
      tool.execute('call-6', { action: 'open_url', url: 'file:///data/local/tmp/token' }),
      /HTTP 或 HTTPS/,
    )
    await assert.rejects(
      tool.execute('call-7', { action: 'open_app', appUrl: 'intent://unsafe' }),
      /URL Scheme/,
    )
    await assert.rejects(
      tool.execute('call-8', { action: 'open_dialer', phoneNumber: '13800;rm -rf' }),
      /电话号码格式无效/,
    )
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})
