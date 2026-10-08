import { createHash } from 'node:crypto'
import { lstat, mkdir, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'

const CATALOG_FILE = 'speech-model-catalog.json'
const NOTICES_FILE = 'speech-resource-notices.json'
const BPE_RESOURCE = 'speech-resources/xasr-bpe.vocab'
const BPE_BYTES = 61562
const BPE_SHA256 = '01381aa0c3065832cb8d7462d529e3079a99be56c955ce93b4cb9b78e8aa34e5'

function validRelativePath(value) {
  return (
    typeof value === 'string' &&
    !/[\\:\p{Cc}]/u.test(value) &&
    value.split('/').every((part) => part && part !== '.' && part !== '..')
  )
}

function verifyCatalog(catalog) {
  const fail = () => {
    throw new Error('Speech catalog structure or resource allowlist is invalid.')
  }
  if (
    catalog?.version !== 1 ||
    !Array.isArray(catalog.models) ||
    catalog.models.length < 1 ||
    catalog.models.length > 32
  )
    fail()
  const ids = new Map()
  for (const model of catalog.models) {
    if (
      !model ||
      !/^[A-Za-z0-9][A-Za-z0-9._-]{0,119}$/.test(model.id || '') ||
      ids.has(model.id) ||
      !['asr', 'tts'].includes(model.kind) ||
      !model.config ||
      !Array.isArray(model.files) ||
      model.files.length === 0
    )
      fail()
    ids.set(model.id, model.kind)
    const paths = new Set()
    for (const file of model.files) {
      if (
        !file ||
        !validRelativePath(file.path) ||
        paths.has(file.path) ||
        !Number.isSafeInteger(file.bytes) ||
        file.bytes < 0 ||
        !/^[a-f0-9]{64}$/.test(file.sha256 || '')
      )
        fail()
      paths.add(file.path)
    }
    if (
      model.config.bpeVocabResource !== undefined &&
      model.config.bpeVocabResource !== BPE_RESOURCE
    )
      fail()
  }
  for (const kind of ['asr', 'tts']) {
    if (ids.get(catalog.defaults?.[kind]) !== kind) fail()
  }
}

async function readSmallFile(path, maxBytes) {
  const info = await lstat(path)
  if (!info.isFile() || info.size > maxBytes) {
    throw new Error(`Speech resource is not a bounded regular file: ${path}`)
  }
  const bytes = await readFile(path)
  if (bytes.length > maxBytes) throw new Error(`Speech resource is too large: ${path}`)
  return bytes
}

function contains(parent, child) {
  const path = relative(parent, child)
  return path === '' || (!isAbsolute(path) && path !== '..' && !path.startsWith(`..${sep}`))
}

async function rejectSymlinkAncestors(path) {
  for (let current = path; ; current = dirname(current)) {
    const info = await lstat(current).catch((error) => {
      if (error.code === 'ENOENT') return null
      throw error
    })
    if (info?.isSymbolicLink()) throw new Error(`Speech staging rejects symlinks: ${current}`)
    if (dirname(current) === current) return
  }
}

export async function stageSpeechResources({ sourceDir, targetDir }) {
  if (
    typeof sourceDir !== 'string' ||
    !sourceDir.trim() ||
    typeof targetDir !== 'string' ||
    !targetDir.trim()
  ) {
    throw new Error('Speech staging requires shared source and assets target directories.')
  }
  const source = await realpath(resolve(sourceDir))
  const target = resolve(targetDir)
  if (
    !['assets', 'SpeechResources'].includes(basename(target)) ||
    contains(source, target) ||
    contains(target, source)
  ) {
    throw new Error('Speech staging target must be a separate assets or SpeechResources root.')
  }
  await rejectSymlinkAncestors(target)
  await rejectSymlinkAncestors(join(target, 'speech-resources'))
  await rejectSymlinkAncestors(join(target, CATALOG_FILE))
  await rejectSymlinkAncestors(join(target, NOTICES_FILE))
  await rejectSymlinkAncestors(join(source, BPE_RESOURCE))
  const catalogBytes = await readSmallFile(join(source, CATALOG_FILE), 4 * 1024 * 1024)
  const catalog = JSON.parse(catalogBytes.toString('utf8'))
  verifyCatalog(catalog)
  const noticesBytes = await readSmallFile(join(source, NOTICES_FILE), 128 * 1024)
  const notices = JSON.parse(noticesBytes.toString('utf8'))
  if (
    notices.schemaVersion !== 1 ||
    !Array.isArray(notices.models) ||
    !catalog.models.every((model) =>
      notices.models.some((notice) => notice.id === model.id && notice.revision === model.revision),
    )
  ) {
    throw new Error('Speech notices do not cover the staged catalog revisions.')
  }
  const bpeBytes = await readSmallFile(join(source, BPE_RESOURCE), BPE_BYTES)
  if (
    bpeBytes.length !== BPE_BYTES ||
    createHash('sha256').update(bpeBytes).digest('hex') !== BPE_SHA256
  ) {
    throw new Error('Speech BPE resource SHA256 or size mismatch.')
  }

  // 先校验全部小资源，再仅清理明确归属语音的 staging 子目录，保留嵌入式 Runtime。
  await mkdir(target, { recursive: true })
  await rm(join(target, 'speech-model'), { recursive: true, force: true })
  await rm(join(target, 'speech-resources'), { recursive: true, force: true })
  await mkdir(join(target, 'speech-resources'), { recursive: true })
  await writeFile(join(target, BPE_RESOURCE), bpeBytes, { flag: 'wx' })
  await rm(join(target, CATALOG_FILE), { force: true })
  await writeFile(join(target, CATALOG_FILE), catalogBytes, { flag: 'wx' })
  await rm(join(target, NOTICES_FILE), { force: true })
  await writeFile(join(target, NOTICES_FILE), noticesBytes, { flag: 'wx' })
  return target
}
