// 部分 TUN 代理把所有域名映射到 198.18/15。只对这种合成结果重新做公网 DNS 查询，
// 不把基准测试地址加入允许列表；连接仍固定在校验过的公网 IP，TLS 继续验证原主机名。
// DoH JSON 协议：https://developers.cloudflare.com/1.1.1.1/encryption/dns-over-https/make-api-requests/dns-json/
import { lookup as dnsLookup } from 'node:dns/promises'
import { isIP } from 'node:net'
import { isBlockedAddress } from './public-address.mjs'

/** @param {string} address */
function synthetic(address) {
  return isIP(address) === 4 && /^198\.(18|19)\./.test(address)
}

/** @param {unknown} value @returns {Record<string, unknown> | null} */
function record(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? /** @type {Record<string, unknown>} */ (value)
    : null
}

/**
 * @param {string} hostname
 * @param {AbortSignal} [signal]
 * @param {{ lookup?: typeof dnsLookup, fetchImpl?: typeof fetch }} [dependencies]
 * @returns {Promise<{address: string, family: number}[]>}
 */
export async function lookupProviderAddresses(
  hostname,
  signal,
  { lookup = dnsLookup, fetchImpl = fetch } = {},
) {
  const addresses = await lookup(hostname, { all: true, verbatim: true })
  if (!addresses.length || !addresses.every((entry) => synthetic(entry.address))) return addresses
  const url = new URL('https://cloudflare-dns.com/dns-query')
  url.searchParams.set('name', hostname)
  url.searchParams.set('type', 'A')
  const response = await fetchImpl(url, {
    headers: { accept: 'application/dns-json' },
    signal,
    redirect: 'error',
    credentials: 'omit',
  })
  if (!response.ok || !response.body) throw new Error('provider_dns_failed')
  const reader = response.body.getReader()
  const chunks = []
  let size = 0
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      size += value.byteLength
      if (size > 64 * 1024) throw new Error('provider_dns_invalid')
      chunks.push(value)
    }
  } catch (error) {
    await reader.cancel().catch(() => {})
    throw error
  }
  const data = record(JSON.parse(Buffer.concat(chunks).toString('utf8')))
  const question =
    Array.isArray(data?.Question) && data.Question.length === 1 ? record(data.Question[0]) : null
  if (
    data?.Status !== 0 ||
    data?.TC === true ||
    typeof question?.name !== 'string' ||
    question.name.replace(/\.$/, '').toLowerCase() !== hostname.toLowerCase() ||
    question.type !== 1 ||
    !Array.isArray(data.Answer) ||
    data.Answer.length > 64
  )
    throw new Error('provider_dns_invalid')
  const result = []
  for (const raw of data.Answer) {
    const entry = record(raw)
    if (entry?.type !== 1) continue
    if (typeof entry.data !== 'string' || isIP(entry.data) !== 4 || isBlockedAddress(entry.data))
      throw new Error('provider_dns_invalid')
    result.push({ address: entry.data, family: 4 })
  }
  if (!result.length) throw new Error('provider_dns_invalid')
  return result
}
