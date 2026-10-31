// 公网地址判定：组件网络代理（custom-ui-fetch-proxy）在 DNS 解析之后用它逐个校验目标地址，
// 防止组件经代理访问本机、内网、链路本地或云元数据服务（SSRF）。
//
// 为什么不只看主机名：攻击者可以让任意域名解析到 127.0.0.1 或 169.254.169.254，
// 也可以在两次解析之间换地址（DNS rebinding）。所以代理先解析、全部校验，
// 再只把校验过的 IP 交给连接；这里负责“哪些 IP 不能连”。
import { BlockList, isIP } from 'node:net'

// 按 IANA 特殊用途地址表收紧：凡是不该从组件访问到的段一律阻断（宁可多拦，不可漏拦）。
/** @type {readonly [string, number][]} */
const BLOCKED_IPV4_SUBNETS = Object.freeze([
  ['0.0.0.0', 8], // “本网络”，在多数系统上等同本机
  ['10.0.0.0', 8], // 私网
  ['100.64.0.0', 10], // 运营商级 NAT（也常被 Tailscale 等内网隧道使用）
  ['127.0.0.0', 8], // 回环
  ['169.254.0.0', 16], // 链路本地，含云元数据 169.254.169.254
  ['172.16.0.0', 12], // 私网
  ['192.0.0.0', 24], // IETF 协议分配
  ['192.0.2.0', 24], // 文档示例
  ['192.88.99.0', 24], // 6to4 中继任播（已废弃）
  ['192.168.0.0', 16], // 私网
  ['198.18.0.0', 15], // 基准测试
  ['198.51.100.0', 24], // 文档示例
  ['203.0.113.0', 24], // 文档示例
  ['224.0.0.0', 4], // 组播
  ['240.0.0.0', 4], // 保留，含 255.255.255.255 广播
])

/** @type {readonly [string, number][]} */
const BLOCKED_IPV6_SUBNETS = Object.freeze([
  ['::', 96], // 未指定、回环 ::1 与已废弃的 IPv4 兼容地址 ::a.b.c.d
  ['64:ff9b::', 96], // NAT64 公共前缀：可映射到任意 IPv4（含内网）
  ['64:ff9b:1::', 48], // NAT64 本地前缀
  ['100::', 64], // 丢弃
  ['2001::', 32], // Teredo：内嵌经混淆的 IPv4
  ['2001:db8::', 32], // 文档示例
  ['2002::', 16], // 6to4：内嵌 IPv4，可能指向内网
  ['fc00::', 7], // 唯一本地地址（私网）
  ['fe80::', 10], // 链路本地
  ['fec0::', 10], // 已废弃的站点本地
  ['ff00::', 8], // 组播
])

// 云厂商元数据服务的主机名：即使 DNS 返回公网地址也不放行（有的环境会被劫持到内部解析）。
const BLOCKED_HOSTNAMES = new Set([
  'localhost',
  'metadata',
  'metadata.google.internal',
  'metadata.goog',
  'metadata.azure.internal',
  'instance-data',
  'instance-data.ec2.internal',
  'kubernetes.default',
  'kubernetes.default.svc',
])
const BLOCKED_HOSTNAME_SUFFIXES = Object.freeze(['.localhost', '.internal', '.local', '.home.arpa'])

/** @returns {BlockList} */
export function createBlockedAddressList() {
  const list = new BlockList()
  for (const [network, prefix] of BLOCKED_IPV4_SUBNETS) list.addSubnet(network, prefix, 'ipv4')
  for (const [network, prefix] of BLOCKED_IPV6_SUBNETS) list.addSubnet(network, prefix, 'ipv6')
  return list
}

const DEFAULT_BLOCK_LIST = createBlockedAddressList()

/**
 * 把 IPv6 文本展开成 8 组 16 位数字；非法时返回 null。支持 :: 压缩与末尾点分 IPv4。
 * @param {string} address
 * @returns {number[] | null}
 */
function expandIpv6(address) {
  let text = address
  /** @type {number[]} */
  let tail = []
  const lastColon = text.lastIndexOf(':')
  const dotted = text.slice(lastColon + 1)
  if (dotted.includes('.')) {
    if (isIP(dotted) !== 4) return null
    const octets = dotted.split('.').map(Number)
    tail = [(octets[0] << 8) | octets[1], (octets[2] << 8) | octets[3]]
    text = `${text.slice(0, lastColon + 1)}${tail.length ? '0:0' : ''}`
  }
  const halves = text.split('::')
  if (halves.length > 2) return null
  const parse = (/** @type {string} */ part) => (part ? part.split(':') : [])
  const head = parse(halves[0])
  const rest = halves.length === 2 ? parse(halves[1]) : []
  const missing = 8 - head.length - rest.length
  if (halves.length === 1 ? missing !== 0 : missing < 1) return null
  const groups = [...head, ...Array(halves.length === 2 ? missing : 0).fill('0'), ...rest]
  if (groups.length !== 8 || !groups.every((group) => /^[0-9a-f]{1,4}$/i.test(group))) return null
  const values = groups.map((group) => Number.parseInt(group, 16))
  if (tail.length) values.splice(6, 2, ...tail)
  return values
}

/**
 * IPv4 映射地址（::ffff:a.b.c.d 及其十六进制写法 ::ffff:7f00:1）展开成 IPv4，再按 IPv4 规则判断；
 * 否则返回 null。
 * @param {string} address
 * @returns {string | null}
 */
export function mappedIpv4(address) {
  const groups = expandIpv6(address)
  if (!groups) return null
  const mapped = groups.slice(0, 5).every((group) => group === 0) && groups[5] === 0xffff
  if (!mapped) return null
  return [groups[6] >> 8, groups[6] & 0xff, groups[7] >> 8, groups[7] & 0xff].join('.')
}

/**
 * 地址是否被阻断。非 IP 文本、带区域标识（fe80::1%eth0）的地址一律视为阻断。
 * @param {string} address
 * @param {{ blockList?: BlockList }} [options]
 * @returns {boolean}
 */
export function isBlockedAddress(address, { blockList = DEFAULT_BLOCK_LIST } = {}) {
  if (typeof address !== 'string' || !address || address.includes('%')) return true
  const raw = address.startsWith('[') && address.endsWith(']') ? address.slice(1, -1) : address
  const family = isIP(raw)
  if (family === 4) return blockList.check(raw, 'ipv4')
  if (family !== 6) return true
  const ipv4 = mappedIpv4(raw)
  if (ipv4) return blockList.check(ipv4, 'ipv4')
  // 不认识的写法（理论上 isIP 已经保证合法）同样按阻断处理。
  if (!expandIpv6(raw)) return true
  return blockList.check(raw, 'ipv6')
}

/**
 * 主机名是否属于元数据服务或本机 / 局域网专用后缀。只接受已经小写、去掉末尾点的主机名。
 * @param {string} hostname
 * @returns {boolean}
 */
export function isBlockedHostname(hostname) {
  if (typeof hostname !== 'string' || !hostname) return true
  const host = hostname.toLowerCase().replace(/\.$/, '')
  if (BLOCKED_HOSTNAMES.has(host)) return true
  return BLOCKED_HOSTNAME_SUFFIXES.some((suffix) => host.endsWith(suffix))
}
