// 移动设备工具：通过当前手机 App 的原生桥访问用户批准的数据、硬件和外部应用。
// Runtime 不直接持有系统权限，所有调用都必须由手机端策略和系统授权放行。
import { randomUUID } from 'node:crypto'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { Type } from 'typebox'
import { defineTool } from '../../runtime/pi-coding-agent.mjs'

export const manifest = {
  id: 'mobile_device',
  name: 'Mobile Device',
  category: 'device',
  risk: 'high',
  description:
    'Use phone capabilities permitted by the connected Pisper mobile App and operating system, including device status, storage, clipboard, photos, sensors, and constrained external apps.',
  scope: 'Currently connected Pisper mobile App and its per-capability privacy controls',
  capability:
    'Inspect or update supported phone state, organize photos, use sensors, and open approved external targets',
  source: 'app',
}

const actionSchema = Type.Union(
  [
    'search_contacts',
    'capture_photo',
    'get_location',
    'get_device_info',
    'get_capabilities',
    'get_battery_status',
    'get_storage_status',
    'get_memory_status',
    'get_network_status',
    'get_display_status',
    'get_locale_status',
    'get_device_status',
    'get_clipboard',
    'set_clipboard',
    'vibrate',
    'set_flashlight',
    'send_notification',
    'list_photos',
    'create_photo_album',
    'add_photos_to_album',
    'delete_photos',
    'share_text',
    'open_url',
    'open_map',
    'open_system_settings',
    'open_dialer',
    'compose_sms',
    'open_app',
  ].map((value) => Type.Literal(value)),
)

const PACKAGE_NAME = /^[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z][A-Za-z0-9_]*)+$/
const PHONE_NUMBER = /^[+*#0-9(). -]{1,64}$/
const FORBIDDEN_APP_SCHEMES = new Set([
  'about:',
  'content:',
  'data:',
  'file:',
  'http:',
  'https:',
  'intent:',
  'javascript:',
])

function requiredText(value, label) {
  const text = String(value || '').trim()
  if (!text) throw new Error(`${label}不能为空。`)
  return text
}

function webUrl(value) {
  const text = requiredText(value, '网址')
  let parsed
  try {
    parsed = new URL(text)
  } catch {
    throw new Error('网址格式无效。')
  }
  if (!['http:', 'https:'].includes(parsed.protocol) || !parsed.hostname) {
    throw new Error('只能打开有效的 HTTP 或 HTTPS 网址。')
  }
  return parsed.href
}

function appUrl(value) {
  const text = requiredText(value, 'App URL')
  let parsed
  try {
    parsed = new URL(text)
  } catch {
    throw new Error('App URL 格式无效。')
  }
  if (FORBIDDEN_APP_SCHEMES.has(parsed.protocol)) {
    throw new Error('App URL 必须使用受支持的第三方应用 URL Scheme。')
  }
  return parsed.href
}

function phoneNumber(value) {
  const text = requiredText(value, '电话号码')
  if (!PHONE_NUMBER.test(text)) throw new Error('电话号码格式无效。')
  return text
}

function externalOperation(params) {
  switch (params.action) {
    case 'open_url':
      return ['apps.open_url', { url: webUrl(params.url) }]
    case 'open_map': {
      const latitude = Number(params.latitude)
      const longitude = Number(params.longitude)
      if (!Number.isFinite(latitude) || latitude < -90 || latitude > 90) {
        throw new Error('纬度必须在 -90 到 90 之间。')
      }
      if (!Number.isFinite(longitude) || longitude < -180 || longitude > 180) {
        throw new Error('经度必须在 -180 到 180 之间。')
      }
      return [
        'apps.open_map',
        {
          latitude,
          longitude,
          ...(params.label ? { label: String(params.label).trim() } : {}),
        },
      ]
    }
    case 'open_system_settings':
      return ['apps.open_system_settings', {}]
    case 'open_dialer':
      return ['apps.open_dialer', { phoneNumber: phoneNumber(params.phoneNumber) }]
    case 'compose_sms':
      return [
        'apps.compose_sms',
        {
          phoneNumber: phoneNumber(params.phoneNumber),
          ...(params.message ? { message: String(params.message) } : {}),
        },
      ]
    case 'open_app': {
      const packageName = String(params.packageName || '').trim()
      const deepLink = String(params.appUrl || '').trim()
      if (!packageName && !deepLink) {
        throw new Error('打开应用需要 Android 包名或 iOS App URL。')
      }
      if (packageName && !PACKAGE_NAME.test(packageName)) {
        throw new Error('Android 应用包名格式无效。')
      }
      return [
        'apps.open_app',
        {
          ...(packageName ? { packageName } : {}),
          ...(deepLink ? { appUrl: appUrl(deepLink) } : {}),
        },
      ]
    }
    default:
      return null
  }
}

function safeImageData(value) {
  const data = String(value || '')
  if (!data || data.length > 12_000_000 || !/^[A-Za-z0-9+/]*={0,2}$/.test(data)) {
    throw new Error('手机返回了无效或过大的照片数据。')
  }
  return Buffer.from(data, 'base64')
}

function photoAssetIds(value) {
  if (!Array.isArray(value) || value.length < 1 || value.length > 100) {
    throw new Error('照片 ID 数量必须在 1 到 100 之间。')
  }
  const ids = [...new Set(value.map((item) => String(item || '').trim()))]
  if (ids.length !== value.length || ids.some((id) => !id || id.length > 256)) {
    throw new Error('照片 ID 无效。')
  }
  return ids
}

function boundedText(value, label, maxLength = 100_000) {
  const text = String(value ?? '')
  if (!text || text.length > maxLength)
    throw new Error(`${label}不能为空且不能超过${maxLength}个字符。`)
  return text
}

function dateFilter(value, label) {
  if (value === undefined) return undefined
  const text = requiredText(value, label)
  const timestamp = Date.parse(text)
  if (!Number.isFinite(timestamp)) throw new Error(`${label}格式无效。`)
  return new Date(timestamp).toISOString()
}

export function createMobileDeviceTool({
  mobileOperationService,
  mobileSessionId,
  mobileCaptureDir,
  onGeneratedFile,
}) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: manifest.description,
    promptSnippet:
      'Use capabilities permitted by the currently connected Pisper mobile App and operating system, including constrained external app launch',
    promptGuidelines: [
      'Call mobile_device only when the user explicitly asks to use data, hardware, or an external app on their current phone.',
      'Use search_contacts for contact lookup, capture_photo for one user-visible camera capture, and get_location for one foreground location fix.',
      'Use get_capabilities before an unfamiliar mobile operation to inspect platform support, permission state, required parameters, and limitations. Use get_device_status for one complete read-only device snapshot, or use get_device_info, get_memory_status, get_storage_status, get_network_status, get_display_status, get_locale_status, and get_battery_status for focused queries. iOS does not expose global free system memory to third-party apps; report the platform limitation instead of guessing.',
      'Use get_clipboard/set_clipboard only for text the user explicitly asks to read or replace. Clipboard reads may show an operating-system privacy notice.',
      'Use list_photos to inspect metadata and create_photo_album for organization. add_photos_to_album moves Android media files and therefore requires explicit confirmation; use delete_photos only after the user explicitly confirms deletion and set confirmed=true.',
      'Photo access is limited by OS permission and Android scoped storage; never claim that every library item was inspected when the result says limited.',
      'Use vibrate and set_flashlight for small, user-requested physical feedback. Flashlight may require camera permission on both platforms.',
      'Use send_notification only for a notification the user explicitly asks Pisper to post; it is local, user-visible, and does not run background work.',
      'Use share_text for a user-visible system share sheet; opening the sheet is not confirmation that content was shared.',
      'Use open_url for HTTP(S), open_map for coordinates, open_system_settings for settings, open_dialer to prefill a phone number, and compose_sms to prefill but never send a message.',
      'Use open_app only with a known Android packageName or iOS custom-scheme appUrl. Opening an app does not authorize interaction inside it; never claim that a song, call, message, payment, or third-party action completed merely because its UI opened.',
      'A specific third-party action such as playing a song is possible only when that app documents a matching deep link or system integration. Never guess a scheme or use accessibility/UI injection.',
      'The phone may reject a capability through Pisper settings or operating-system permissions. Report that refusal without trying to bypass it.',
      'Never infer that SMS reading exists. Play-compatible Pisper builds can only open a user-visible SMS composer and do not request SMS permissions.',
      'Keep contact queries, photo limits, and asset ID batches narrow when the task does not require a broad scan.',
    ],
    parameters: Type.Object({
      action: actionSchema,
      query: Type.Optional(
        Type.String({ maxLength: 120, description: 'Name or phone fragment for search_contacts' }),
      ),
      limit: Type.Optional(
        Type.Integer({ minimum: 1, maximum: 200, description: 'Maximum contacts to return' }),
      ),
      cameraDirection: Type.Optional(Type.Union([Type.Literal('back'), Type.Literal('front')])),
      url: Type.Optional(Type.String({ maxLength: 2_048 })),
      latitude: Type.Optional(Type.Number({ minimum: -90, maximum: 90 })),
      longitude: Type.Optional(Type.Number({ minimum: -180, maximum: 180 })),
      label: Type.Optional(Type.String({ maxLength: 120 })),
      phoneNumber: Type.Optional(Type.String({ maxLength: 64 })),
      message: Type.Optional(Type.String({ maxLength: 2_000 })),
      text: Type.Optional(Type.String({ maxLength: 100_000 })),
      title: Type.Optional(Type.String({ maxLength: 120 })),
      packageName: Type.Optional(Type.String({ maxLength: 200 })),
      appUrl: Type.Optional(Type.String({ maxLength: 2_048 })),
      intensity: Type.Optional(
        Type.Union([Type.Literal('light'), Type.Literal('medium'), Type.Literal('heavy')]),
      ),
      durationMs: Type.Optional(Type.Integer({ minimum: 10, maximum: 2_000 })),
      body: Type.Optional(Type.String({ maxLength: 4_000 })),
      notifyId: Type.Optional(Type.String({ maxLength: 120 })),

      enabled: Type.Optional(Type.Boolean()),
      albumId: Type.Optional(Type.String({ maxLength: 256 })),
      albumName: Type.Optional(Type.String({ maxLength: 120 })),
      assetIds: Type.Optional(
        Type.Array(Type.String({ maxLength: 256 }), { minItems: 1, maxItems: 100 }),
      ),
      confirmed: Type.Optional(Type.Boolean()),
      mediaType: Type.Optional(Type.Union([Type.Literal('image'), Type.Literal('video')])),
      fromDate: Type.Optional(Type.String({ maxLength: 64 })),
      toDate: Type.Optional(Type.String({ maxLength: 64 })),
    }),
    async execute(_toolCallId, params, signal) {
      if (!mobileOperationService) throw new Error('移动设备操作服务未初始化。')
      const external = externalOperation(params)
      const operation =
        external?.[0] ||
        {
          search_contacts: 'contacts.search',
          capture_photo: 'camera.capture',
          get_location: 'location.current',
          get_device_info: 'device.info',
          get_capabilities: 'device.capabilities',
          get_battery_status: 'device.battery',
          get_storage_status: 'device.storage',
          get_memory_status: 'device.memory',
          get_network_status: 'device.network',
          get_display_status: 'device.display',
          get_locale_status: 'device.locale',
          get_device_status: 'device.status',
          get_clipboard: 'device.clipboard.get',
          set_clipboard: 'device.clipboard.set',
          vibrate: 'device.vibrate',
          set_flashlight: 'device.flashlight',
          send_notification: 'device.notify',
          list_photos: 'photos.list',
          create_photo_album: 'photos.create_album',
          add_photos_to_album: 'photos.add_to_album',
          delete_photos: 'photos.delete',
          share_text: 'apps.share_text',
        }[params.action]
      if (!operation) throw new Error('不支持的移动设备操作。')
      if (
        (params.action === 'add_photos_to_album' || params.action === 'delete_photos') &&
        params.confirmed !== true
      ) {
        throw new Error('移动或删除照片前必须获得用户明确确认。')
      }
      const assetIds =
        params.action === 'add_photos_to_album' || params.action === 'delete_photos'
          ? photoAssetIds(params.assetIds)
          : undefined
      const parameters = external?.[1] || {
        ...(params.query ? { query: params.query } : {}),
        ...(params.limit ? { limit: params.limit } : {}),
        ...(params.cameraDirection ? { cameraDirection: params.cameraDirection } : {}),
        ...(params.text !== undefined ? { text: boundedText(params.text, '文本') } : {}),
        ...(params.title ? { title: String(params.title).trim() } : {}),
        ...(params.body !== undefined ? { body: boundedText(params.body, '通知内容', 4_000) } : {}),
        ...(params.notifyId ? { notifyId: String(params.notifyId).trim() } : {}),
        ...(params.intensity ? { intensity: params.intensity } : {}),
        ...(params.durationMs ? { durationMs: params.durationMs } : {}),
        ...(params.enabled !== undefined ? { enabled: params.enabled } : {}),
        ...(params.albumId ? { albumId: String(params.albumId).trim() } : {}),
        ...(params.albumName ? { albumName: String(params.albumName).trim() } : {}),
        ...(assetIds ? { assetIds } : {}),
        ...(params.confirmed !== undefined ? { confirmed: params.confirmed } : {}),
        ...(params.mediaType ? { mediaType: params.mediaType } : {}),
        ...(params.fromDate ? { fromDate: dateFilter(params.fromDate, '起始日期') } : {}),
        ...(params.toDate ? { toDate: dateFilter(params.toDate, '结束日期') } : {}),
      }
      const result = await mobileOperationService.execute(mobileSessionId, operation, parameters, {
        signal,
      })

      if (params.action !== 'capture_photo') {
        return {
          content: [{ type: 'text', text: JSON.stringify(result, null, 2) }],
          details: result,
        }
      }

      const image = safeImageData(result?.data)
      await mkdir(mobileCaptureDir, { recursive: true })
      const path = join(mobileCaptureDir, `mobile-photo-${Date.now()}-${randomUUID()}.jpg`)
      await writeFile(path, image, { mode: 0o600 })
      const details = {
        path,
        mimeType: 'image/jpeg',
        width: Number(result?.width || 0),
        height: Number(result?.height || 0),
        size: image.length,
      }
      await onGeneratedFile?.(details)
      return {
        content: [
          {
            type: 'text',
            text: `Captured photo saved to ${path} (${details.width}x${details.height}, ${details.size} bytes).`,
          },
          { type: 'image', data: String(result.data), mimeType: 'image/jpeg' },
        ],
        details,
      }
    },
  })
}
