// iOS 嵌入式 Node 只能来自固定发布：归档摘要、Sigstore 身份、内部清单和
// xcframework 逐文件摘要全部通过后才会进入 Tauri staging。
import { execFile } from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  cp,
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  stat,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, join, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const run = promisify(execFile)
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const metadata = JSON.parse(
  await readFile(join(root, 'scripts', 'mobile-node-artifacts.json'), 'utf8'),
)
const outputArgument = process.argv.slice(2).find((argument) => !argument.startsWith('--'))
const output = resolve(outputArgument || join(root, 'release', 'mobile-node-ios'))
const requireSigstore = process.argv.includes('--require-sigstore')
const cacheDir = resolve(
  process.env.PISPER_NODE_MOBILE_CACHE_DIR || join(root, 'release', 'mobile-node-downloads'),
)
const releaseBase = `${metadata.ios.repository}/releases/download/${metadata.ios.releaseTag}`
const archive = join(cacheDir, metadata.ios.archive)
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
    metadata.ios.certificateIdentityRegexp,
    '--certificate-oidc-issuer',
    metadata.ios.certificateOidcIssuer,
    archive,
  ])
}

function assertArchiveEntries(entries) {
  for (const entry of entries) {
    const normalized = entry.replaceAll('\\', '/')
    if (
      !normalized ||
      normalized.startsWith('/') ||
      /^[A-Za-z]:\//.test(normalized) ||
      normalized.split('/').includes('..')
    ) {
      throw new Error(`iOS Node 归档包含非法路径：${entry}`)
    }
  }
}

async function listFiles(path, base = path) {
  const files = []
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const target = join(path, entry.name)
    if (entry.isDirectory()) files.push(...(await listFiles(target, base)))
    else if (entry.isFile()) files.push(relative(base, target).split(sep).join('/'))
    else throw new Error(`iOS Node 归档包含不支持的文件类型：${target}`)
  }
  return files
}

async function verifyInternalDigests(staging) {
  const lines = (await readFile(join(staging, 'SHA256SUMS'), 'utf8')).split(/\r?\n/).filter(Boolean)
  const expected = new Map()
  for (const line of lines) {
    const match = /^([a-f0-9]{64})  (.+)$/.exec(line)
    if (!match) throw new Error(`iOS Node SHA256SUMS 行格式无效：${line}`)
    const path = match[2].replaceAll('\\', '/')
    if (path.startsWith('/') || path.split('/').includes('..') || expected.has(path)) {
      throw new Error(`iOS Node SHA256SUMS 路径无效：${path}`)
    }
    expected.set(path, match[1])
  }

  const coveredRoots = ['NodeMobile.xcframework', 'include']
  const actualFiles = (
    await Promise.all(
      coveredRoots.map(async (path) =>
        (await listFiles(join(staging, path))).map((file) => `${path}/${file}`),
      ),
    )
  )
    .flat()
    .sort()
  assertEqual(expected.size, actualFiles.length, 'SHA256SUMS entry count')
  for (const path of actualFiles) {
    const expectedDigest = expected.get(path)
    if (!expectedDigest) throw new Error(`iOS Node SHA256SUMS 缺少文件：${path}`)
    const actual = await digest(join(staging, ...path.split('/')))
    if (actual !== expectedDigest) throw new Error(`iOS Node 文件 SHA256 不匹配：${path}`)
  }
}

function assertEqual(actual, expected, label) {
  if (actual !== expected) throw new Error(`iOS Node ${label} 不匹配：${actual}`)
}

await Promise.all([
  download(`${releaseBase}/${metadata.ios.archive}`, archive, metadata.ios.archiveSha256),
  download(
    `${releaseBase}/${metadata.ios.archive}.sigstore.json`,
    bundle,
    metadata.ios.sigstoreBundleSha256,
  ),
])
await verifySigstore()

const staging = await mkdtemp(join(tmpdir(), 'pisper-node-ios-'))
try {
  const { stdout: archiveEntries } = await run('unzip', ['-Z1', archive], {
    maxBuffer: 16 * 1024 * 1024,
  })
  assertArchiveEntries(archiveEntries.split(/\r?\n/).filter(Boolean))
  await run('unzip', ['-q', archive, '-d', staging])
  await verifyInternalDigests(staging)

  const manifest = JSON.parse(await readFile(join(staging, 'runtime-manifest.json'), 'utf8'))
  const assertions = [
    [manifest.schemaVersion, 1, 'manifest schema'],
    [manifest.releaseTag, metadata.ios.releaseTag, 'release tag'],
    [manifest.sourceRepository, metadata.ios.repository, 'release repository'],
    [manifest.sourceCommit, metadata.ios.sourceCommit, 'release source commit'],
    [manifest.recipe?.repository, metadata.source.repository, 'recipe repository'],
    [manifest.recipe?.commit, metadata.source.commit, 'recipe commit'],
    [manifest.recipe?.tree, metadata.source.recipeTree, 'recipe tree'],
    [manifest.recipe?.materializedTree, metadata.source.materializedTree, 'source tree'],
    [manifest.recipe?.upstreamCommit, metadata.source.upstreamCommit, 'upstream commit'],
    [manifest.runtime?.nodeVersion, metadata.runtime.nodeVersion, 'Node version'],
    [Number(manifest.runtime?.modulesAbi), metadata.runtime.modulesAbi, 'modules ABI'],
    [manifest.platform, 'ios', 'platform'],
    [manifest.toolchain?.runner, 'macos-26', 'runner'],
  ]
  for (const [actual, expected, label] of assertions) assertEqual(actual, expected, label)
  assertEqual(
    JSON.stringify(manifest.architectures),
    JSON.stringify(['arm64', 'arm64-simulator']),
    'architectures',
  )
  if (!String(manifest.toolchain?.xcode || '').startsWith('Xcode ')) {
    throw new Error('iOS Node Xcode 元数据无效。')
  }

  const frameworkRoot = join(staging, 'NodeMobile.xcframework')
  const frameworkFiles = await listFiles(frameworkRoot)
  const binaries = frameworkFiles.filter((path) => path.endsWith('/NodeMobile'))
  assertEqual(binaries.length, 2, 'framework slice count')
  if (!frameworkFiles.includes('Info.plist'))
    throw new Error('iOS Node 缺少 xcframework Info.plist。')

  await rm(output, { recursive: true, force: true })
  await mkdir(output, { recursive: true })
  await Promise.all([
    cp(frameworkRoot, join(output, 'NodeMobile.xcframework'), { recursive: true, force: true }),
    cp(join(staging, 'include'), join(output, 'include'), { recursive: true, force: true }),
  ])
  await Promise.all([
    copyFile(join(staging, 'LICENSE.nodejs'), join(output, 'LICENSE.nodejs')),
    copyFile(join(staging, 'runtime-manifest.json'), join(output, 'runtime-manifest.json')),
    copyFile(join(staging, 'SHA256SUMS'), join(output, 'SHA256SUMS')),
  ])
  await writeFile(
    join(output, 'pisper-node-artifact.json'),
    `${JSON.stringify(
      {
        schemaVersion: metadata.schemaVersion,
        source: metadata.source,
        runtime: metadata.runtime,
        releaseTag: metadata.ios.releaseTag,
        sourceCommit: metadata.ios.sourceCommit,
        archive: metadata.ios.archive,
        archiveSha256: metadata.ios.archiveSha256,
        sigstoreBundleSha256: metadata.ios.sigstoreBundleSha256,
        sigstoreVerified: requireSigstore,
      },
      null,
      2,
    )}\n`,
  )
  const bytes = (
    await Promise.all(binaries.map((path) => stat(join(frameworkRoot, ...path.split('/')))))
  ).reduce((total, entry) => total + entry.size, 0)
  console.log(`iOS embedded Node staged: ${output} (${bytes} framework bytes)`)
} finally {
  await rm(staging, { recursive: true, force: true })
}
