// 远程接入点枚举：列出本机可被移动端连接的地址（局域网 IPv4、公网 IPv6、
// Tailscale 网卡），供配对二维码与 mDNS 广播使用。
import { hostname, networkInterfaces } from 'node:os'

function isPrivateIpv4(address) {
  const parts = address.split('.').map(Number)
  if (parts.length !== 4 || parts.some((part) => Number.isNaN(part))) return false
  return (
    parts[0] === 10 ||
    (parts[0] === 172 && parts[1] >= 16 && parts[1] <= 31) ||
    (parts[0] === 192 && parts[1] === 168)
  )
}

// Tailscale 的 CGNAT 网段：100.64.0.0/10。
function isTailscaleIpv4(address) {
  const parts = address.split('.').map(Number)
  if (parts.length !== 4 || parts.some((part) => Number.isNaN(part))) return false
  return parts[0] === 100 && parts[1] >= 64 && parts[1] <= 127
}

function classifyIpv6(address) {
  const normalized = address.toLowerCase()
  // 链路本地地址离开本机不可达，跳过。
  if (normalized.startsWith('fe80:') || normalized.startsWith('fe80::')) return null
  // Teredo（2001:0::/32）与 6to4（2002::/16）是隧道伪地址，通常不可被直连，跳过。
  if (normalized.startsWith('2001:0:') || normalized.startsWith('2001::')) return null
  if (normalized.startsWith('2002:')) return null
  // ULA（fc00::/7）仅本地路由，归为局域网档。
  if (normalized.startsWith('fc') || normalized.startsWith('fd')) return 'lan'
  return 'v6'
}

// 汇总当前可用于远程接入的 endpoint 列表，按 lan → v6 → ts 的建议优先级排序。
export function collectRemoteEndpoints({ port, tls = true } = {}) {
  const scheme = tls ? 'https' : 'http'
  const endpoints = []
  const interfaces = networkInterfaces()
  for (const infos of Object.values(interfaces)) {
    for (const info of infos || []) {
      if (!info || info.internal) continue
      if (info.family === 'IPv4') {
        if (isTailscaleIpv4(info.address)) {
          endpoints.push({ t: 'ts', url: `${scheme}://${info.address}:${port}` })
        } else if (isPrivateIpv4(info.address)) {
          endpoints.push({ t: 'lan', url: `${scheme}://${info.address}:${port}` })
        }
      } else if (info.family === 'IPv6') {
        const kind = classifyIpv6(info.address)
        if (kind) endpoints.push({ t: kind, url: `${scheme}://[${info.address}]:${port}` })
      }
    }
  }
  const order = { lan: 0, v6: 1, ts: 2 }
  endpoints.sort((left, right) => (order[left.t] ?? 9) - (order[right.t] ?? 9))
  return endpoints
}

// 二维码/mDNS 里展示的设备名：取主机名，兜底一个可读名称。
export function remoteDeviceName() {
  try {
    return hostname() || 'Pisper Desktop'
  } catch {
    return 'Pisper Desktop'
  }
}
