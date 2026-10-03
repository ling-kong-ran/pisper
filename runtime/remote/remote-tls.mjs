// 远程访问 TLS：首次启用远程模式时生成自签证书并持久化到数据目录，
// 指纹保持稳定（仅当文件被删除或损坏时重新生成）。客户端通过配对二维码
// 带外获得证书指纹（TOFU），不依赖系统 CA。
import { X509Certificate } from 'node:crypto'
import { mkdirSync, readFileSync, writeFileSync, chmodSync } from 'node:fs'
import { join } from 'node:path'
import selfsigned from 'selfsigned'

// 把 node:crypto 的指纹格式（AA:BB:...）归一化为契约格式（SHA256:AABB...）。
export function normalizeFingerprint(fingerprint256) {
  return `SHA256:${String(fingerprint256 || '')
    .replaceAll(':', '')
    .toUpperCase()}`
}

export function certificateFingerprint(certPem) {
  return normalizeFingerprint(new X509Certificate(certPem).fingerprint256)
}

// 读取或生成远程访问证书。返回 PEM 密钥/证书与指纹。
export async function ensureRemoteCertificate({ dataDir }) {
  const filePath = join(dataDir, 'remote-tls.json')
  try {
    const stored = JSON.parse(readFileSync(filePath, 'utf8'))
    if (stored && typeof stored.cert === 'string' && typeof stored.key === 'string') {
      // 解析一遍既验证 PEM 有效性，也顺便算出指纹。
      const fingerprint = certificateFingerprint(stored.cert)
      return { key: stored.key, cert: stored.cert, fingerprint }
    }
  } catch {
    // 缺失或损坏时重新生成；损坏文件会被覆盖（其中不含其他有价值数据）。
  }
  // selfsigned v5 为异步 API（内部走 WebCrypto 生成密钥对）。
  const pems = await selfsigned.generate([{ name: 'commonName', value: 'Pisper Remote' }], {
    // 自签证书即自己的根：客户端按指纹锁定这一张证书，CA 位让严格校验路径也能通过。
    days: 3650,
    keySize: 2048,
    algorithm: 'sha256',
    extensions: [
      { name: 'basicConstraints', cA: true },
      {
        name: 'subjectAltName',
        altNames: [
          { type: 2, value: 'localhost' },
          { type: 7, ip: '127.0.0.1' },
          { type: 7, ip: '::1' },
        ],
      },
    ],
  })
  const fingerprint = certificateFingerprint(pems.cert)
  mkdirSync(dataDir, { recursive: true })
  writeFileSync(
    filePath,
    `${JSON.stringify({ version: 1, cert: pems.cert, key: pems.private }, null, 2)}\n`,
    'utf8',
  )
  try {
    // 私钥落盘需限制权限。
    chmodSync(filePath, 0o600)
  } catch {
    // Windows 不支持 POSIX 权限位，忽略。
  }
  return { key: pems.private, cert: pems.cert, fingerprint }
}
