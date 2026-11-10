// 密码登录时认主机的方式：用户确认过的那把主机公钥，之后只认它。
// 密钥登录第一次连接“对方是谁就信谁”问题不大（私钥不会交出去）；密码登录不行——第一次连接要是被人截在中间，
// 密码就直接交给了冒充的服务器。所以密码机器的公钥由用户先核对指纹，存在机器记录上，连接时写进只有这一行的
// known_hosts，ssh 以 StrictHostKeyChecking=yes 只认它。纯函数，按不可信数据校验。
import { createHash } from 'node:crypto'

// 连接时用固定的 HostKeyAlias 查 known_hosts：文件里只有一行，不用拼 [host]:port 的写法，
// ssh 的密码提示里写的主机名也是它（user@pisper-pinned's password:）。
export const HOST_KEY_ALIAS = 'pisper-pinned'

const HOST_KEY_TYPES = new Set([
  'ssh-ed25519',
  'ecdsa-sha2-nistp256',
  'ecdsa-sha2-nistp384',
  'ecdsa-sha2-nistp521',
  'ssh-rsa',
  'sk-ssh-ed25519@openssh.com',
  'sk-ecdsa-sha2-nistp256@openssh.com',
])
const MAX_KEY_CHARS = 4_096
const BASE64_PATTERN = /^[A-Za-z0-9+/]+={0,2}$/

/**
 * 一把主机公钥（类型 + base64 的公钥本体）。公钥本体开头写着它自己的类型，对不上就不是合法的公钥。
 * 指纹从公钥算出来，不信传进来的值。不合法时返回 null。
 * @param {unknown} value
 * @returns {{ type: string, key: string, fingerprint: string } | null}
 */
export function normalizeHostKey(value) {
  if (!value || typeof value !== 'object') return null
  const type = String(value.type ?? '')
  const key = String(value.key ?? '')
  if (!HOST_KEY_TYPES.has(type)) return null
  if (!key || key.length > MAX_KEY_CHARS || key.length % 4 !== 0 || !BASE64_PATTERN.test(key))
    return null
  const blob = Buffer.from(key, 'base64')
  if (blob.length < 4 + type.length) return null
  const length = blob.readUInt32BE(0)
  if (length !== type.length || blob.subarray(4, 4 + length).toString('latin1') !== type)
    return null
  return {
    type,
    key,
    fingerprint: `SHA256:${createHash('sha256').update(blob).digest('base64').replace(/=+$/, '')}`,
  }
}

export function sameHostKey(left, right) {
  return Boolean(left && right) && left.type === right.type && left.key === right.key
}

// 只有这一行的 known_hosts。
export function knownHostsLine(hostKey) {
  return `${HOST_KEY_ALIAS} ${hostKey.type} ${hostKey.key}\n`
}

// ssh 握手后写下的 known_hosts（取指纹时用的临时文件）里，属于这次连接的那把公钥。
export function parseKnownHosts(text) {
  for (const line of String(text).split(/\r?\n/)) {
    const [host, type, key] = line.trim().split(/\s+/)
    if (host !== HOST_KEY_ALIAS) continue
    const hostKey = normalizeHostKey({ type, key })
    if (hostKey) return hostKey
  }
  return null
}
