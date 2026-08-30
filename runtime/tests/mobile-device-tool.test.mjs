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
  // 工作区预设包含生图与手机操作（产物落在工作区/本地设备，随写权限开放）
  assert.equal(TOOL_PRESETS.workspace.includes('mobile_device'), true)
  assert.equal(TOOL_PRESETS.workspace.includes('generate_visual'), true)
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
        if (operation === 'device.storage') {
          return {
            totalBytes: 256_000_000_000,
            availableBytes: 128_000_000_000,
            usedBytes: 128_000_000_000,
            scope: 'device_volume',
          }
        }
        if (operation === 'device.status') {
          return {
            device: { platform: 'ios', model: 'iPhone' },
            memory: { totalBytes: 8_000_000_000, availableBytes: null },
            storage: { totalBytes: 256_000_000_000, availableBytes: 128_000_000_000 },
            battery: { level: 0.8, status: 'charging' },
            network: { connected: true, transport: 'wifi' },
            display: { widthPixels: 1179, heightPixels: 2556 },
            locale: { languageTag: 'zh-CN', timeZone: 'Asia/Shanghai' },
            platformLimitations: ['iOS 不提供全局可用系统内存。'],
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

    await tool.execute('call-9', { action: 'get_device_info' })
    assert.deepEqual(calls[5], {
      sessionId: 'session-mobile',
      operation: 'device.info',
      parameters: {},
    })

    await tool.execute('call-10', { action: 'set_clipboard', text: '待粘贴内容' })
    assert.deepEqual(calls[6], {
      sessionId: 'session-mobile',
      operation: 'device.clipboard.set',
      parameters: { text: '待粘贴内容' },
    })

    await tool.execute('call-11', {
      action: 'list_photos',
      query: 'trip',
      limit: 12,
      fromDate: '2026-01-01',
      toDate: '2026-01-31T23:59:59Z',
    })
    assert.deepEqual(calls[7], {
      sessionId: 'session-mobile',
      operation: 'photos.list',
      parameters: {
        query: 'trip',
        limit: 12,
        fromDate: '2026-01-01T00:00:00.000Z',
        toDate: '2026-01-31T23:59:59.000Z',
      },
    })

    await assert.rejects(
      tool.execute('call-12', {
        action: 'add_photos_to_album',
        albumId: 'Pictures/Trips/',
        assetIds: ['42', '43'],
      }),
      /明确确认/,
    )
    assert.equal(calls.length, 8)
    await tool.execute('call-13', {
      action: 'add_photos_to_album',
      albumId: 'Pictures/Trips/',
      assetIds: ['42', '43'],
      confirmed: true,
    })
    assert.deepEqual(calls[8], {
      sessionId: 'session-mobile',
      operation: 'photos.add_to_album',
      parameters: { albumId: 'Pictures/Trips/', assetIds: ['42', '43'], confirmed: true },
    })

    await assert.rejects(
      tool.execute('call-14', { action: 'delete_photos', assetIds: ['42'] }),
      /明确确认/,
    )
    assert.equal(calls.length, 9)
    await tool.execute('call-15', {
      action: 'delete_photos',
      assetIds: ['42'],
      confirmed: true,
    })
    assert.deepEqual(calls[9], {
      sessionId: 'session-mobile',
      operation: 'photos.delete',
      parameters: { assetIds: ['42'], confirmed: true },
    })

    await tool.execute('call-16', {
      action: 'send_notification',
      title: 'Pisper 测试',
      body: '请在手机上查看通知',
      notifyId: 'test-notification',
    })
    assert.deepEqual(calls[10], {
      sessionId: 'session-mobile',
      operation: 'device.notify',
      parameters: {
        title: 'Pisper 测试',
        body: '请在手机上查看通知',
        notifyId: 'test-notification',
      },
    })

    await tool.execute('call-17', { action: 'share_text', text: '请在手机上选择分享目标' })
    assert.deepEqual(calls[11], {
      sessionId: 'session-mobile',
      operation: 'apps.share_text',
      parameters: { text: '请在手机上选择分享目标' },
    })

    const capabilities = await tool.execute('call-18', { action: 'get_capabilities' })
    assert.match(capabilities.content[0].text, /contacts/)
    assert.deepEqual(calls[12], {
      sessionId: 'session-mobile',
      operation: 'device.capabilities',
      parameters: {},
    })

    const storage = await tool.execute('call-19', { action: 'get_storage_status' })
    assert.match(storage.content[0].text, /availableBytes/)
    assert.deepEqual(calls[13], {
      sessionId: 'session-mobile',
      operation: 'device.storage',
      parameters: {},
    })

    for (const [callId, action, operation, index] of [
      ['call-20', 'get_memory_status', 'device.memory', 14],
      ['call-21', 'get_network_status', 'device.network', 15],
      ['call-22', 'get_display_status', 'device.display', 16],
      ['call-23', 'get_locale_status', 'device.locale', 17],
    ]) {
      await tool.execute(callId, { action })
      assert.deepEqual(calls[index], {
        sessionId: 'session-mobile',
        operation,
        parameters: {},
      })
    }

    const deviceStatus = await tool.execute('call-24', { action: 'get_device_status' })
    assert.match(deviceStatus.content[0].text, /platformLimitations/)
    assert.deepEqual(calls[18], {
      sessionId: 'session-mobile',
      operation: 'device.status',
      parameters: {},
    })
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})
