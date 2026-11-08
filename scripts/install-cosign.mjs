// 发布工作流的 cosign 备用安装。sigstore/cosign-installer 只从 github.com 的 releases/download 下载，
// 那个入口整段时间回 504 时整步失败（2026-11-08 移动端发版卡在这里）。这里装同一个版本：
// 经 github-download.mjs 下载（入口不通时走 GitHub API），按仓库里固定的 SHA-256 校验后放进 PATH。
// 摘要取自 cosign v2.6.1 发布的 cosign_checksums.txt。
import { createHash } from 'node:crypto'
import { appendFile, chmod, mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { downloadReleaseAsset } from './github-download.mjs'

export const COSIGN_VERSION = 'v2.6.1'
export const COSIGN_ASSETS = {
  'linux-x64': {
    name: 'cosign-linux-amd64',
    sha256: '064954c5d8c7e3b28188eee5b1727b31c411550bc5fefd41aa672d3c761d103a',
  },
  'darwin-arm64': {
    name: 'cosign-darwin-arm64',
    sha256: '54047052cf46f40a5c3c95a510db276e164ba77e096aea1ca1b733f770359689',
  },
}
const MAX_COSIGN_BYTES = 300 * 1024 * 1024

export async function installCosign({
  dir,
  platform = process.platform,
  arch = process.arch,
  fetchImpl = globalThis.fetch,
  retryDelayMs,
}) {
  const asset = COSIGN_ASSETS[`${platform}-${arch}`]
  if (!asset) throw new Error(`No pinned cosign build for ${platform}-${arch}`)
  const url = `https://github.com/sigstore/cosign/releases/download/${COSIGN_VERSION}/${asset.name}`
  const bytes = await downloadReleaseAsset(url, {
    fetchImpl,
    ...(retryDelayMs === undefined ? {} : { retryDelayMs }),
    timeoutMs: 600_000,
    maxBytes: MAX_COSIGN_BYTES,
    verify: (value) => {
      const actual = createHash('sha256').update(value).digest('hex')
      if (actual !== asset.sha256) throw new Error(`${asset.name} SHA256 mismatch: ${actual}`)
    },
  })
  await mkdir(dir, { recursive: true })
  const binary = join(dir, 'cosign')
  await writeFile(binary, bytes)
  await chmod(binary, 0o755)
  return binary
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const dir = process.argv[2] || join(process.env.RUNNER_TEMP || '.', 'cosign-bin')
  const binary = await installCosign({ dir })
  if (process.env.GITHUB_PATH) await appendFile(process.env.GITHUB_PATH, `${dir}\n`)
  console.log(`Installed cosign ${COSIGN_VERSION} at ${binary}`)
}
