// 本机应用请求判定：连接来自回环地址、Host 为本机名且 Origin 一致，并且不是配对设备的远程请求。
// 用于只能由本机桌面/浏览器触发的管理或系统操作，防止远端设备与 DNS 重绑定越权。
export function isLocalAppRequest(req) {
  const address = String(req.socket?.remoteAddress || '').replace(/^::ffff:/, '')
  const loopback = address === '::1' || /^127\.(?:\d{1,3}\.){2}\d{1,3}$/.test(address)
  let localHost = false
  try {
    const host = new URL(`http://${req.headers.host}`)
    localHost = ['127.0.0.1', 'localhost', '[::1]'].includes(host.hostname)
    if (req.headers.origin && req.headers.origin !== host.origin) localHost = false
  } catch {
    // 无法校验 Host 的请求不视为本机请求。
  }
  return !req.pisperRemote && loopback && localHost
}
