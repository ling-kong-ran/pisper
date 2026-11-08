// 移动设备工具：通过当前手机 App 的原生桥访问用户批准的数据、硬件和外部应用。
// Runtime 不直接持有系统权限，所有调用都必须由手机端策略和系统授权放行。
import { randomUUID } from 'node:crypto'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { Type } from 'typebox'
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { pisperError } from '../../../shared/errors.mjs'
import { MOBILE_DEVICE } from '../../prompts/tools/mobile-device.mjs'

export const manifest = {
  id: 'mobile_device',
  name: 'Mobile Device',
  category: 'device',
  risk: 'high',
  description: MOBILE_DEVICE.description,
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

function requiredText(value, field) {
  const text = String(value || '').trim()
  if (!text) throw pisperError('invalid', 'input', `${field} is required`)
  return text
}

function webUrl(value) {
  const text = requiredText(value, 'url')
  let parsed
  try {
    parsed = new URL(text)
  } catch {
    throw pisperError('invalid', 'url', 'use an http or https URL')
  }
  if (!['http:', 'https:'].includes(parsed.protocol) || !parsed.hostname) {
    throw pisperError('invalid', 'url', 'use an http or https URL')
  }
  return parsed.href
}

function appUrl(value) {
  const text = requiredText(value, 'appUrl')
  let parsed
  try {
    parsed = new URL(text)
  } catch {
    throw pisperError('invalid', 'url', 'appUrl must use a third-party app URL scheme')
  }
  if (FORBIDDEN_APP_SCHEMES.has(parsed.protocol)) {
    throw pisperError('invalid', 'url', 'appUrl must use a third-party app URL scheme')
  }
  return parsed.href
}

function phoneNumber(value) {
  const text = requiredText(value, 'phoneNumber')
  if (!PHONE_NUMBER.test(text)) throw pisperError('invalid', 'input', 'phoneNumber')
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
        throw pisperError('invalid', 'input', 'latitude must be -90 to 90')
      }
      if (!Number.isFinite(longitude) || longitude < -180 || longitude > 180) {
        throw pisperError('invalid', 'input', 'longitude must be -180 to 180')
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
        throw pisperError('invalid', 'input', 'packageName or appUrl is required')
      }
      if (packageName && !PACKAGE_NAME.test(packageName)) {
        throw pisperError('invalid', 'input', 'packageName')
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
    throw pisperError('invalid', 'image', 'the phone returned invalid or oversized photo data')
  }
  return Buffer.from(data, 'base64')
}

function photoAssetIds(value) {
  if (!Array.isArray(value) || value.length < 1 || value.length > 100) {
    throw pisperError('invalid', 'input', 'assetIds must have 1 to 100 items')
  }
  const ids = [...new Set(value.map((item) => String(item || '').trim()))]
  if (ids.length !== value.length || ids.some((id) => !id || id.length > 256)) {
    throw pisperError('invalid', 'input', 'assetIds')
  }
  return ids
}

function boundedText(value, field, maxLength = 100_000) {
  const text = String(value ?? '')
  if (!text || text.length > maxLength)
    throw pisperError('invalid', 'input', `${field} must be 1 to ${maxLength} characters`)
  return text
}

function dateFilter(value, field) {
  if (value === undefined) return undefined
  const text = requiredText(value, field)
  const timestamp = Date.parse(text)
  if (!Number.isFinite(timestamp)) throw pisperError('invalid', 'input', field)
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
    promptSnippet: MOBILE_DEVICE.snippet,
    promptGuidelines: MOBILE_DEVICE.guidelines,
    parameters: Type.Object({
      action: actionSchema,
      query: Type.Optional(
        Type.String({ maxLength: 120, description: MOBILE_DEVICE.params.query }),
      ),
      limit: Type.Optional(
        Type.Integer({ minimum: 1, maximum: 200, description: MOBILE_DEVICE.params.limit }),
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
      if (!mobileOperationService) throw pisperError('unavailable', 'device')
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
      if (!operation) throw pisperError('unsupported', 'operation', params.action)
      if (
        (params.action === 'add_photos_to_album' || params.action === 'delete_photos') &&
        params.confirmed !== true
      ) {
        throw pisperError(
          'invalid',
          'operation',
          'set confirmed=true only after the user explicitly confirms',
        )
      }
      const assetIds =
        params.action === 'add_photos_to_album' || params.action === 'delete_photos'
          ? photoAssetIds(params.assetIds)
          : undefined
      const parameters = external?.[1] || {
        ...(params.query ? { query: params.query } : {}),
        ...(params.limit ? { limit: params.limit } : {}),
        ...(params.cameraDirection ? { cameraDirection: params.cameraDirection } : {}),
        ...(params.text !== undefined ? { text: boundedText(params.text, 'text') } : {}),
        ...(params.title ? { title: String(params.title).trim() } : {}),
        ...(params.body !== undefined ? { body: boundedText(params.body, 'body', 4_000) } : {}),
        ...(params.notifyId ? { notifyId: String(params.notifyId).trim() } : {}),
        ...(params.intensity ? { intensity: params.intensity } : {}),
        ...(params.durationMs ? { durationMs: params.durationMs } : {}),
        ...(params.enabled !== undefined ? { enabled: params.enabled } : {}),
        ...(params.albumId ? { albumId: String(params.albumId).trim() } : {}),
        ...(params.albumName ? { albumName: String(params.albumName).trim() } : {}),
        ...(assetIds ? { assetIds } : {}),
        ...(params.confirmed !== undefined ? { confirmed: params.confirmed } : {}),
        ...(params.mediaType ? { mediaType: params.mediaType } : {}),
        ...(params.fromDate ? { fromDate: dateFilter(params.fromDate, 'fromDate') } : {}),
        ...(params.toDate ? { toDate: dateFilter(params.toDate, 'toDate') } : {}),
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
