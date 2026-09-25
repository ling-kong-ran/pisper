// 保持短于服务端的 60 字符上限；getRandomValues 也适用于局域网 HTTP 页面。
export function createProviderConnectionId(): string {
  const values = globalThis.crypto.getRandomValues(new Uint32Array(4))
  return `custom-${Array.from(values, (value) => value.toString(16).padStart(8, '0')).join('')}`
}
