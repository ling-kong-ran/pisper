import { createWriteStream } from 'node:fs'
import { access, mkdir, rm, cp } from 'node:fs/promises'
import { join, relative, resolve } from 'node:path'
import { pipeline } from 'node:stream/promises'
import { Readable } from 'node:stream'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'

const execFileAsync = promisify(execFile)
const MODEL_ARCHIVE_URL =
  'https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-small-ctc-zh-int8-2025-04-01.tar.bz2'
const MODEL_FILES = ['model.int8.onnx', 'tokens.txt']

async function hasModelFiles(directory) {
  try {
    await Promise.all(MODEL_FILES.map((file) => access(join(directory, file))))
    return true
  } catch {
    return false
  }
}

async function copyModel(source, target) {
  if (!(await hasModelFiles(source))) return false
  await mkdir(target, { recursive: true })
  await Promise.all(MODEL_FILES.map((file) => cp(join(source, file), join(target, file))))
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
    // Git for Windows 的 tar 会把带盘符的绝对路径误判为远程归档，改用相对路径并固定工作目录。
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
