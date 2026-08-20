// 远程访问服务：管理远程模式的开关、一次性配对码与已配对设备令牌。
// 持久化到 <dataDir>/remote-access.json；设备令牌只存 SHA-256 哈希，
// 所有密钥比较走常量时间比较，避免时序侧信道。
import { createHash, randomBytes, timingSafeEqual } from 'node:crypto'
import { chmodSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'

export const PAIRING_CODE_TTL_MS = 5 * 60 * 1000
export const PAIRING_RATE_LIMIT_WINDOW_MS = 60 * 1000
export const PAIRING_RATE_LIMIT_MAX_FAILURES = 5

const STORE_VERSION = 1
// Crockford Base32 字母表：去掉易混淆的 I/L/O/U，便于人工核对与口述。
const CODE_ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'
const CODE_LENGTH = 8

export class RemoteAccessError extends Error {
  constructor(code, message) {
    super(message)
    this.name = 'RemoteAccessError'
    this.code = code
  }
}

function hashSecret(value) {
  return createHash('sha256').update(String(value)).digest('hex')
}

// 比较两个 hex 摘要：长度一致时用常量时间比较。
function secretEquals(leftValue, rightHash) {
  const left = Buffer.from(hashSecret(leftValue), 'utf8')
  const right = Buffer.from(String(rightHash || ''), 'utf8')
  return left.length === right.length && timingSafeEqual(left, right)
}

// 归一化用户输入的配对码：忽略大小写与连字符等分隔符。
export function normalizePairingCode(input) {
  return String(input || '')
    .toUpperCase()
    .replace(/[^0-9A-Z]/g, '')
}

// 展示用格式：ABCD-EFGH。
export function formatPairingCode(normalized) {
  const code = normalizePairingCode(normalized)
  return code.length > 4 ? `${code.slice(0, 4)}-${code.slice(4)}` : code
}

// 生成 8 字符配对码：256 是 32 的整数倍，取模无偏。
function generatePairingCode() {
  const bytes = randomBytes(CODE_LENGTH)
  let code = ''
  for (const byte of bytes) code += CODE_ALPHABET[byte % CODE_ALPHABET.length]
  return code
}

// 对外暴露的设备信息：永不包含令牌哈希。
function publicDevice(device, currentId = null) {
  return {
    id: device.id,
    name: device.name,
    createdAt: device.createdAt,
    lastSeenAt: device.lastSeenAt,
    revokedAt: device.revokedAt,
    current: device.id === currentId,
  }
}

export class RemoteAccessService {
  constructor({ dataDir, filePath = null } = {}) {
    this.filePath = filePath || join(dataDir, 'remote-access.json')
    this.state = { version: STORE_VERSION, enabled: false, devices: [], pairingCode: null }
    // 配对失败限流：按来源 IP 统计，窗口内连续失败即冷却（仅内存，不落盘）。
    this.failures = new Map()
    // 已认证设备的活跃响应：吊销时用于主动断开其 SSE 长连接。
    this.activeResponses = new Map()
    this.load()
  }

  load() {
    try {
      const parsed = JSON.parse(readFileSync(this.filePath, 'utf8'))
      if (parsed && typeof parsed === 'object') {
        this.state.enabled = Boolean(parsed.enabled)
        if (Array.isArray(parsed.devices)) {
          this.state.devices = parsed.devices.filter(
            (device) => device && typeof device === 'object' && device.id && device.tokenHash,
          )
        }
      }
    } catch {
      // 文件缺失或损坏时从空状态开始；损坏文件不覆盖，便于人工排查。
    }
  }

  save() {
    const temporaryPath = `${this.filePath}.${process.pid}.tmp`
    mkdirSync(dirname(this.filePath), { recursive: true })
    writeFileSync(
      temporaryPath,
      `${JSON.stringify(
        {
          version: STORE_VERSION,
          enabled: this.state.enabled,
          devices: this.state.devices,
          pairingCode: this.state.pairingCode,
        },
        null,
        2,
      )}\n`,
      'utf8',
    )
    // 临时文件 + rename 保证写入原子性，避免断电留下半截 JSON。
    renameSync(temporaryPath, this.filePath)
    try {
      // 文件内含令牌哈希与配对码哈希，限制为仅当前用户可读。
      chmodSync(this.filePath, 0o600)
    } catch {
      // Windows 不支持 POSIX 权限位，忽略。
    }
  }

  isEnabled() {
    return this.state.enabled
  }

  setEnabled(enabled) {
    this.state.enabled = Boolean(enabled)
    this.save()
    return this.state.enabled
  }

  // 签发一次性配对码：作废旧码，保证任意时刻至多一个有效配对码。
  issuePairingCode() {
    const code = generatePairingCode()
    const expiresAt = new Date(Date.now() + PAIRING_CODE_TTL_MS).toISOString()
    this.state.pairingCode = { codeHash: hashSecret(code), expiresAt }
    this.save()
    return { code: formatPairingCode(code), expiresAt }
  }

  rateLimitState(ip) {
    const now = Date.now()
    const record = this.failures.get(ip)
    if (record && now >= record.resetAt) {
      this.failures.delete(ip)
      return null
    }
    return record || null
  }

  recordFailure(ip) {
    const now = Date.now()
    const record = this.rateLimitState(ip) || {
      count: 0,
      resetAt: now + PAIRING_RATE_LIMIT_WINDOW_MS,
    }
    record.count += 1
    record.resetAt = now + PAIRING_RATE_LIMIT_WINDOW_MS
    this.failures.set(ip, record)
  }

  assertNotRateLimited(ip) {
    const record = this.rateLimitState(ip)
    if (record && record.count >= PAIRING_RATE_LIMIT_MAX_FAILURES) {
      throw new RemoteAccessError('pairing_rate_limited', '配对失败次数过多，请稍后再试。')
    }
  }

  // 用配对码换取长期设备令牌：成功即消费配对码（一次性）。
  redeemPairingCode({ code, deviceName, ip = 'unknown' } = {}) {
    this.assertNotRateLimited(ip)
    const fail = (error) => {
      this.recordFailure(ip)
      throw error
    }
    const pending = this.state.pairingCode
    if (!pending) {
      fail(new RemoteAccessError('pairing_code_invalid', '配对码不正确，请核对后重试。'))
    }
    if (Date.parse(pending.expiresAt) < Date.now()) {
      fail(new RemoteAccessError('pairing_code_expired', '配对码已过期，请在桌面端重新生成。'))
    }
    if (!secretEquals(normalizePairingCode(code), pending.codeHash)) {
      fail(new RemoteAccessError('pairing_code_invalid', '配对码不正确，请核对后重试。'))
    }
    this.state.pairingCode = null
    this.failures.delete(ip)
    const now = new Date().toISOString()
    // 令牌明文只在此时生成并返回一次；服务端只保存哈希。
    const token = `pst_${randomBytes(24).toString('base64url')}`
    const device = {
      id: `dev_${randomBytes(9).toString('base64url')}`,
      name:
        String(deviceName || '')
          .trim()
          .slice(0, 80) || '未命名设备',
      tokenHash: hashSecret(token),
      createdAt: now,
      lastSeenAt: now,
      revokedAt: null,
    }
    this.state.devices.push(device)
    this.save()
    return { device: publicDevice(device), token }
  }

  // 校验 Bearer 令牌：返回未吊销的设备，或 null。
  authenticateToken(token) {
    if (!token) return null
    for (const device of this.state.devices) {
      if (device.revokedAt) continue
      if (secretEquals(String(token), device.tokenHash)) {
        // lastSeenAt 刷新节流：最多每分钟写一次盘。
        const now = Date.now()
        if (now - Date.parse(device.lastSeenAt || 0) > 60_000) {
          device.lastSeenAt = new Date(now).toISOString()
          this.save()
        }
        return device
      }
    }
    return null
  }

  listDevices(currentId = null) {
    return this.state.devices.map((device) => publicDevice(device, currentId))
  }

  getDevice(id) {
    return this.state.devices.find((device) => device.id === id) || null
  }

  // 吊销设备：新请求立即 401；该设备的活跃连接（如 SSE）被主动断开。
  // except：发起吊销的请求自身不在断开之列，保证它能正常返回 204。
  revokeDevice(id, { except = null } = {}) {
    const device = this.state.devices.find((item) => item.id === id && !item.revokedAt)
    if (!device) throw new RemoteAccessError('device_not_found', '设备不存在或已被吊销。')
    device.revokedAt = new Date().toISOString()
    this.save()
    for (const res of this.activeResponses.get(id) || []) {
      if (res === except) continue
      try {
        res.end()
      } catch {
        // 响应可能已结束，忽略。
      }
    }
    this.activeResponses.delete(id)
    return publicDevice(device)
  }

  // 跟踪已认证设备持有的活跃响应；响应关闭时自动解除跟踪。
  trackResponse(deviceId, res) {
    let set = this.activeResponses.get(deviceId)
    if (!set) {
      set = new Set()
      this.activeResponses.set(deviceId, set)
    }
    set.add(res)
    res.on('close', () => {
      set.delete(res)
      if (!set.size) this.activeResponses.delete(deviceId)
    })
  }
}

// 从请求头提取 Bearer 令牌。
export function bearerToken(req) {
  const header = String(req.headers.authorization || '')
  const match = /^Bearer\s+(.+)$/i.exec(header)
  return match ? match[1].trim() : ''
}
