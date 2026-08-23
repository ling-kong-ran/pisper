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
    'Access user-approved contacts, camera, foreground location, or constrained external apps.',
  scope: 'Currently connected Pisper mobile App and its per-capability privacy controls',
  capability:
    'Search contacts, capture one photo, read one foreground location fix, or open an approved external target',
  source: 'app',
}

const actionSchema = Type.Union(
  [
    'search_contacts',
    'capture_photo',
    'get_location',
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
      'Use user-approved capabilities on the currently connected Pisper mobile App, including constrained external app launch',
    promptGuidelines: [
      'Call mobile_device only when the user explicitly asks to use data, hardware, or an external app on their current phone.',
      'Use search_contacts for contact lookup or organization, capture_photo for one user-visible camera capture, and get_location for one foreground location fix.',
      'Use open_url for HTTP(S), open_map for coordinates, open_system_settings for settings, open_dialer to prefill a phone number, and compose_sms to prefill but never send a message.',
      'Use open_app only with a known Android packageName or iOS custom-scheme appUrl. Never guess an identifier when the target app is ambiguous.',
      'Opening an app does not authorize interaction inside it. Never claim that a call, message, payment, or third-party action completed merely because its UI opened.',
      'The phone may reject a capability through Pisper settings or operating-system permissions. Report that refusal without trying to bypass it.',
      'Never infer that SMS reading exists. Play-compatible Pisper builds can only open a user-visible SMS composer and do not request SMS permissions.',
      'Keep contact queries and limits narrow when the task does not require the complete contact list.',
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
      packageName: Type.Optional(Type.String({ maxLength: 200 })),
      appUrl: Type.Optional(Type.String({ maxLength: 2_048 })),
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
        }[params.action]
      const parameters = external?.[1] || {
        ...(params.query ? { query: params.query } : {}),
        ...(params.limit ? { limit: params.limit } : {}),
        ...(params.cameraDirection ? { cameraDirection: params.cameraDirection } : {}),
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
