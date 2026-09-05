import { createReadStream, createWriteStream } from 'node:fs'
import { access, mkdir, rm, cp } from 'node:fs/promises'
import { join, relative, resolve } from 'node:path'
import { pipeline } from 'node:stream/promises'
import { Readable } from 'node:stream'
import { execFile } from 'node:child_process'
import { createHash } from 'node:crypto'
import { promisify } from 'node:util'
import { exportSpeechBpeVocab } from './speech-bpe.mjs'

const execFileAsync = promisify(execFile)
const MODEL_ARCHIVE_URL =
  'https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-x-asr-480ms-streaming-zipformer-transducer-zh-en-punct-int8-2026-06-05.tar.bz2'
const MODEL_ARCHIVE_SHA256 = 'fa5f63d618e5a01526e275a358bb7772e403f84808a4769fba52cffd8160bf74'
const MODEL_FILES = [
  'encoder.int8.onnx',
  'decoder.onnx',
  'joiner.int8.onnx',
  'tokens.txt',
  'bpe.model',
]

async function hasModelFiles(directory) {
  try {
    await Promise.all(MODEL_FILES.map((file) => access(join(directory, file))))
    return true
  } catch {
    return false
  }
}

async function sha256File(path) {
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(path)) hash.update(chunk)
  return hash.digest('hex')
}

async function copyModel(source, target) {
  if (!(await hasModelFiles(source))) return false
  await mkdir(target, { recursive: true })
  await Promise.all(MODEL_FILES.map((file) => cp(join(source, file), join(target, file))))
  await exportSpeechBpeVocab(join(target, 'bpe.model'), join(target, 'bpe.vocab'))
  return true
}

export async function stageSpeechModel({ root, runtimeDir }) {
  const target = join(resolve(runtimeDir), 'runtime', 'speech-model')
  const configuredSource = String(process.env.PISPER_SPEECH_MODEL_DIR || '').trim()
  if (configuredSource && (await copyModel(resolve(configuredSource), target))) return target

  const cacheDir = join(resolve(root), 'release', 'cache', 'speech-model')
  if (await copyModel(cacheDir, target)) return target

  const archivePath = join(resolve(root), 'release', 'cache', 'speech-model.tar.bz2')
  const extractDir = join(resolve(root), 'release', 'cache', 'speech-model-extract')
  await mkdir(join(resolve(root), 'release', 'cache'), { recursive: true })
  await rm(extractDir, { recursive: true, force: true })
  await mkdir(extractDir, { recursive: true })
  try {
    const response = await fetch(MODEL_ARCHIVE_URL)
    if (!response.ok || !response.body)
      throw new Error(`官方语音模型下载失败 (${response.status || 'network'})。`)
    await pipeline(Readable.fromWeb(response.body), createWriteStream(archivePath))
    if ((await sha256File(archivePath)) !== MODEL_ARCHIVE_SHA256)
      throw new Error('官方语音模型校验失败。')
    // 归档以模型目录开头，剥离一层即可；相对路径同时避免 Git for Windows 把盘符误判为远程地址。
    await execFileAsync(
      'tar',
      [
        '-xjf',
        relative(resolve(root), archivePath),
        '--strip-components=1',
        '-C',
        relative(resolve(root), extractDir),
      ],
      { cwd: resolve(root) },
    )
    if (!(await copyModel(extractDir, cacheDir))) throw new Error('官方语音模型文件不完整。')
    await copyModel(cacheDir, target)
    return target
  } finally {
    await rm(extractDir, { recursive: true, force: true })
    await rm(archivePath, { force: true })
  }
}

export { MODEL_ARCHIVE_URL }
