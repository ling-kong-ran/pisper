// Android 嵌入式 Node 只能来自固定发布：归档摘要、Sigstore 身份、内部清单和
// libnode 摘要全部通过后才会进入 Gradle staging，避免构建时信任移动标签。
import { execFile } from 'node:child_process'
import { createHash } from 'node:crypto'
import { cp, copyFile, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const run = promisify(execFile)
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const metadata = JSON.parse(
  await readFile(join(root, 'scripts', 'mobile-node-artifacts.json'), 'utf8'),
)
const output = resolve(process.argv[2] || join(root, 'release', 'mobile-node-android'))
const requireSigstore = process.argv.includes('--require-sigstore')
const cacheDir = resolve(
  process.env.PISPER_NODE_MOBILE_CACHE_DIR || join(root, 'release', 'mobile-node-downloads'),
)
const releaseBase = `${metadata.source.repository}/releases/download/${metadata.android.releaseTag}`
const archive = join(cacheDir, metadata.android.archive)
const bundle = `${archive}.sigstore.json`

async function digest(path) {
  const hash = createHash('sha256')
  hash.update(await readFile(path))
  return hash.digest('hex')
}

async function download(url, target, expectedSha256) {
  try {
    if ((await digest(target)) === expectedSha256) return
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }
  await mkdir(dirname(target), { recursive: true })
  const response = await fetch(url, { redirect: 'follow' })
  if (!response.ok) throw new Error(`无法下载 ${url}：HTTP ${response.status}`)
  await writeFile(target, Buffer.from(await response.arrayBuffer()))
  const actual = await digest(target)
  if (actual !== expectedSha256) {
    await rm(target, { force: true })
    throw new Error(`${basename(target)} SHA256 不匹配：${actual}`)
  }
}

async function verifySigstore() {
  if (!requireSigstore) return
  await run('cosign', [
    'verify-blob',
    '--bundle',
    bundle,
    '--certificate-identity-regexp',
    metadata.android.certificateIdentityRegexp,
    '--certificate-oidc-issuer',
    metadata.android.certificateOidcIssuer,
    archive,
  ])
}

await Promise.all([
  download(`${releaseBase}/${metadata.android.archive}`, archive, metadata.android.archiveSha256),
  download(
    `${releaseBase}/${metadata.android.archive}.sigstore.json`,
    bundle,
    metadata.android.sigstoreBundleSha256,
  ),
])
await verifySigstore()

const staging = await mkdtemp(join(tmpdir(), 'pisper-node-android-'))
try {
  await run('unzip', ['-q', archive, '-d', staging])
  const manifest = JSON.parse(await readFile(join(staging, 'runtime-manifest.json'), 'utf8'))
  const assertions = [
    [manifest.sourceCommit, metadata.source.commit, 'sourceCommit'],
    [manifest.upstream?.commit, metadata.source.upstreamCommit, 'upstream.commit'],
    [manifest.upstream?.expectedSourceTree, metadata.source.materializedTree, 'source tree'],
    [manifest.runtime?.nodeVersion, metadata.runtime.nodeVersion, 'Node version'],
    [Number(manifest.runtime?.modulesAbi), metadata.runtime.modulesAbi, 'modules ABI'],
    [manifest.toolchain?.targetAbi, 'arm64-v8a', 'target ABI'],
    [manifest.libnodeSha256, metadata.android.libnodeSha256, 'manifest libnode SHA256'],
  ]
  for (const [actual, expected, label] of assertions) {
    if (actual !== expected) throw new Error(`Android Node ${label} 不匹配：${actual}`)
  }
  const library = join(staging, 'bin', 'arm64-v8a', 'libnode.so')
  if ((await digest(library)) !== metadata.android.libnodeSha256) {
    throw new Error('Android libnode.so SHA256 不匹配。')
  }

  await rm(output, { recursive: true, force: true })
  await mkdir(join(output, 'arm64-v8a'), { recursive: true })
  await Promise.all([
    copyFile(library, join(output, 'arm64-v8a', 'libnode.so')),
    cp(join(staging, 'include'), join(output, 'include'), { recursive: true, force: true }),
  ])
  await Promise.all([
    copyFile(join(staging, 'LICENSE'), join(output, 'LICENSE.nodejs')),
    copyFile(join(staging, 'runtime-manifest.json'), join(output, 'runtime-manifest.json')),
  ])
  await writeFile(
    join(output, 'pisper-node-artifact.json'),
    `${JSON.stringify(
      {
        schemaVersion: metadata.schemaVersion,
        source: metadata.source,
        runtime: metadata.runtime,
        releaseTag: metadata.android.releaseTag,
        archive: metadata.android.archive,
        archiveSha256: metadata.android.archiveSha256,
        sigstoreBundleSha256: metadata.android.sigstoreBundleSha256,
        libnodeSha256: metadata.android.libnodeSha256,
        sigstoreVerified: requireSigstore,
      },
      null,
      2,
    )}\n`,
  )
  const bytes = (await stat(join(output, 'arm64-v8a', 'libnode.so'))).size
  console.log(`Android embedded Node staged: ${output} (${bytes} bytes)`)
} finally {
  await rm(staging, { recursive: true, force: true })
}
