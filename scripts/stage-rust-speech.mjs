import { constants } from 'node:fs'
import { lstat, mkdir, open, rename, unlink } from 'node:fs/promises'
import { createHash, randomUUID } from 'node:crypto'
import { dirname, join, relative, resolve, sep } from 'node:path'

const SHERPA_VERSION = '1.13.7'
const ORT_VERSION = '1.27.1'
const PINNED = [
  {
    file: 'sherpa-onnx-c-api.dll',
    bytes: 4593664,
    sha256: 'f91af9cefbdaef81bd9ec012069da2a0bc7ba8961cf0bd01b12373bd5f105bf8',
    version: SHERPA_VERSION,
    license: 'Apache-2.0; compiled dependency audit pending',
  },
  {
    file: 'onnxruntime.dll',
    bytes: 17378304,
    sha256: 'b9f6713c3602a4742680a7e6a77e3f9ac4a676ad9447bce609e53efcda795d7e',
    version: ORT_VERSION,
    license: 'MIT; see ONNXRUNTIME-THIRD-PARTY-NOTICES.txt',
  },
  {
    file: 'onnxruntime_providers_shared.dll',
    bytes: 104960,
    sha256: '1f8b8ebfb07aad2c692669ba77c45b5a24e72b9c708bc3f2be4103ff29370c60',
    version: ORT_VERSION,
    license: 'MIT; see ONNXRUNTIME-THIRD-PARTY-NOTICES.txt',
  },
]
const TEXT = [
  {
    source: 'runtime-rs/src/native_speech/SHERPA-LICENSE',
    path: 'speech-native/SHERPA-LICENSE',
    bytes: 11358,
    sha256: 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30',
    version: SHERPA_VERSION,
    license: 'Apache-2.0',
    origin: 'https://raw.githubusercontent.com/k2-fsa/sherpa-onnx/v1.13.7/LICENSE',
  },
  {
    source: 'runtime-rs/resources/licenses/onnxruntime-1.27.1-LICENSE.txt',
    path: 'speech-native/ONNXRUNTIME-LICENSE.txt',
    bytes: 1073,
    sha256: '2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c',
    version: ORT_VERSION,
    license: 'MIT',
    origin: 'https://raw.githubusercontent.com/microsoft/onnxruntime/v1.27.1/LICENSE',
  },
  {
    source: 'runtime-rs/resources/licenses/THIRD-PARTY-NOTICES.txt',
    path: 'speech-native/ONNXRUNTIME-THIRD-PARTY-NOTICES.txt',
    bytes: 325054,
    sha256: '0e07b95f3a8d6230037707c5c4a2b554d12c4cb67369669ac255635528ffcee2',
    version: ORT_VERSION,
    license: 'Upstream third-party notices; component-specific licenses',
    origin: 'https://raw.githubusercontent.com/microsoft/onnxruntime/v1.27.1/ThirdPartyNotices.txt',
  },
  {
    source: 'shared/speech-resources/xasr-bpe.vocab',
    path: 'shared/speech-resources/xasr-bpe.vocab',
    bytes: 61562,
    sha256: '01381aa0c3065832cb8d7462d529e3079a99be56c955ce93b4cb9b78e8aa34e5',
    version: 'x-asr-480ms-int8-2026-06-05',
    license: 'See shared/speech/speech-resource-notices.json; provenance pending',
  },
]
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex')
const identity = (stat) =>
  [stat.dev, stat.ino, stat.size, stat.mtimeNs, stat.ctimeNs, stat.nlink].join(':')

async function inspect(path) {
  try {
    return await lstat(path, { bigint: true })
  } catch (error) {
    if (error.code === 'ENOENT') return null
    throw error
  }
}

async function rejectLinks(path) {
  let current = resolve(path)
  for (;;) {
    const stat = await inspect(current)
    if (stat?.isSymbolicLink()) throw new Error(`Speech resource path is a link: ${current}`)
    const parent = dirname(current)
    if (parent === current) break
    if (stat && current !== resolve(path) && !stat.isDirectory())
      throw new Error(`Speech resource ancestor is not a directory: ${current}`)
    current = parent
  }
}

async function readBounded(path, limit) {
  await rejectLinks(path)
  const before = await inspect(path)
  // nlink===1 在 POSIX 上是硬链接攻击面的护栏;Windows 卷(GH runner 的
  // 工作区卷)可能对普通文件报 nlink>1,那里靠 O_NOFOLLOW + 打开后
  // 身份比对(含 nlink)保证读取期间未被替换。
  if (
    !before?.isFile() ||
    before.size > BigInt(limit) ||
    (process.platform !== 'win32' && before.nlink !== 1n)
  )
    throw new Error(`Speech resource is not a bounded, independent file: ${path}`)
  const file = await open(path, constants.O_RDONLY | (constants.O_NOFOLLOW || 0))
  try {
    if (identity(before) !== identity(await file.stat({ bigint: true })))
      throw new Error(`Speech resource changed while opening: ${path}`)
    const bytes = Buffer.alloc(Number(before.size))
    let offset = 0
    while (offset < bytes.length) {
      const result = await file.read(bytes, offset, bytes.length - offset, offset)
      if (!result.bytesRead) throw new Error(`Speech resource was truncated: ${path}`)
      offset += result.bytesRead
    }
    const extra = Buffer.alloc(1)
    const readExtra = await file.read(extra, 0, 1, offset)
    await rejectLinks(path)
    const after = await inspect(path)
    if (
      readExtra.bytesRead ||
      !after ||
      identity(before) !== identity(after) ||
      identity(before) !== identity(await file.stat({ bigint: true }))
    )
      throw new Error(`Speech resource changed while reading: ${path}`)
    return bytes
  } finally {
    await file.close()
  }
}

function child(root, path) {
  const result = resolve(root, path)
  const inside = relative(root, result)
  if (!inside || inside === '..' || inside.startsWith(`..${sep}`) || resolve(inside) === inside)
    throw new Error('Speech resource destination is outside its staging root.')
  return result
}

async function atomicWrite(target, bytes) {
  await rejectLinks(target)
  await mkdir(dirname(target), { recursive: true })
  await rejectLinks(target)
  const old = await inspect(target)
  if (old && (!old.isFile() || old.nlink !== 1n))
    throw new Error(`Speech staging destination is not an independent file: ${target}`)
  const temporary = `${target}.${randomUUID()}.tmp`
  let created = false
  try {
    const file = await open(temporary, 'wx', 0o600)
    created = true
    try {
      await file.writeFile(bytes)
      await file.sync()
    } finally {
      await file.close()
    }
    await rejectLinks(target)
    await rename(temporary, target)
  } finally {
    if (created)
      await unlink(temporary).catch((error) => {
        if (error.code !== 'ENOENT') throw error
      })
  }
}

/** Stage the pinned Windows x64 Rust speech closure; never copies a user's downloaded models. */
export async function stageRustSpeech({ root, targetDir }) {
  if (process.platform !== 'win32') throw new Error('Rust speech staging supports Windows x64.')
  if (
    typeof root !== 'string' ||
    !root.trim() ||
    typeof targetDir !== 'string' ||
    !targetDir.trim()
  )
    throw new Error('Rust speech staging requires root and targetDir.')
  root = resolve(root)
  targetDir = resolve(targetDir)
  if (root === targetDir) throw new Error('Speech staging root must differ from the source root.')
  await rejectLinks(root)
  await rejectLinks(targetDir)
  const packagePath = join(root, 'node_modules/sherpa-onnx-win-x64/package.json')
  const metadata = JSON.parse((await readBounded(packagePath, 64 * 1024)).toString('utf8'))
  if (
    metadata.name !== 'sherpa-onnx-win-x64' ||
    metadata.version !== SHERPA_VERSION ||
    !metadata.os?.includes('win32') ||
    !metadata.cpu?.includes('x64')
  )
    throw new Error('Native speech source must be sherpa-onnx-win-x64 1.13.7.')

  const definitions = [
    ...PINNED.map((file) => ({
      ...file,
      source: `node_modules/sherpa-onnx-win-x64/${file.file}`,
      path: `speech-native/${file.file}`,
      origin: 'sherpa-onnx-win-x64@1.13.7',
    })),
    ...TEXT,
    ...['speech-model-catalog.json', 'speech-resource-notices.json'].map((name) => ({
      source: `shared/speech/${name}`,
      path: `shared/speech/${name}`,
      limit: 128 * 1024,
      version: 'shared-catalog-v1',
      license: 'Component declarations and pending provenance in speech-resource-notices.json',
    })),
  ]
  // Validate every source and destination before writing any owned staging file.
  const prepared = []
  for (const definition of definitions) {
    const source = child(root, definition.source)
    const destination = child(targetDir, definition.path)
    if (source === destination) throw new Error('Speech staging cannot overwrite source resources.')
    await rejectLinks(destination)
    const existing = await inspect(destination)
    if (existing && (!existing.isFile() || existing.nlink !== 1n))
      throw new Error(`Speech staging destination is not an independent file: ${destination}`)
    const bytes = await readBounded(source, definition.bytes ?? definition.limit)
    const digest = sha256(bytes)
    if (
      (definition.bytes !== undefined && bytes.length !== definition.bytes) ||
      (definition.sha256 !== undefined && digest !== definition.sha256)
    )
      throw new Error(`Pinned speech resource size or SHA256 mismatch: ${definition.source}`)
    prepared.push({ definition, destination, bytes, digest })
  }
  const jsonResource = (name) =>
    JSON.parse(prepared.find((item) => item.definition.path.endsWith(name)).bytes.toString('utf8'))
  const catalog = jsonResource('speech-model-catalog.json')
  const notices = jsonResource('speech-resource-notices.json')
  if (
    catalog.version !== 1 ||
    !Array.isArray(catalog.models) ||
    !catalog.models.length ||
    notices.schemaVersion !== 1 ||
    !Array.isArray(notices.models) ||
    !catalog.models.every((model) =>
      notices.models.some((notice) => notice.id === model.id && notice.revision === model.revision),
    ) ||
    !catalog.models.some(
      (model) => model.config?.bpeVocabResource === 'speech-resources/xasr-bpe.vocab',
    )
  )
    throw new Error('Shared speech catalog, BPE reference, or provenance notices are inconsistent.')
  const manifest = {
    schemaVersion: 1,
    platform: 'win32',
    arch: 'x64',
    engine: { sherpa: SHERPA_VERSION, onnxruntime: ORT_VERSION },
    resources: prepared.map(({ definition, bytes, digest }) => ({
      path: definition.path,
      source: definition.source,
      version: definition.version,
      bytes: bytes.length,
      sha256: digest,
      license: definition.license,
      ...(definition.origin ? { origin: definition.origin } : {}),
    })),
    licenseAudit: {
      status: 'pending',
      scope:
        'Native compiled or embedded dependencies and source-delivery closure remain pending; shared model notices retain their individual pending evidence.',
    },
  }
  const manifestPath = child(targetDir, 'speech-native/resources.json')
  await rejectLinks(manifestPath)
  const existingManifest = await inspect(manifestPath)
  if (existingManifest && (!existingManifest.isFile() || existingManifest.nlink !== 1n))
    throw new Error('Speech staging manifest destination is not an independent file.')
  for (const { destination, bytes } of prepared) await atomicWrite(destination, bytes)
  await atomicWrite(manifestPath, Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`))
  return manifest
}
