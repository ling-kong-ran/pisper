import { createHash } from 'node:crypto'
import { createReadStream } from 'node:fs'
import { access, copyFile, mkdir, rm } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { exportSpeechBpeVocab } from './speech-bpe.mjs'

const MODEL_FILES = [
  {
    name: 'encoder.int8.onnx',
    sha256: '908596dcc137a73b95be908ca55e88caa1b3dbbe8027c171615f4b0609c5eb1e',
  },
  {
    name: 'decoder.onnx',
    sha256: 'a1cbc9eac2d5e3fb6617a218c67ad6daaa7f4e0fd225f08b2c22ab0413c8c257',
  },
  {
    name: 'joiner.int8.onnx',
    sha256: 'aedb7fa697b2ab43f20499826fff7c997eea7d67db77be97769aeeeb726e63b3',
  },
  {
    name: 'tokens.txt',
    sha256: 'b818a60878b9aae978cbb8ad594acbd403d76d1af2e31ef4197c84e2dbdba27c',
  },
  {
    name: 'bpe.model',
    sha256: 'f87a38025a5fdd1e4e9591f6a44bb81295097ce0b80df6f4ab9f44e52c64ca5f',
  },
]

async function sha256(path) {
  const digest = createHash('sha256')
  for await (const chunk of createReadStream(path)) digest.update(chunk)
  return digest.digest('hex')
}

async function verifyModelDirectory(sourceDir) {
  for (const file of MODEL_FILES) {
    const path = join(sourceDir, file.name)
    await access(path).catch(() => {
      throw new Error(`Android 语音模型缺少 ${file.name}。`)
    })
    if ((await sha256(path)) !== file.sha256) {
      throw new Error(`Android 语音模型 ${file.name} SHA256 不匹配。`)
    }
  }
}

export async function stageAndroidSpeechModel({ sourceDir, targetDir }) {
  const source = String(sourceDir || process.env.PISPER_SPEECH_MODEL_DIR || '').trim()
  if (!source) throw new Error('Android 构建需要有效的语音模型来源。')
  const resolvedSource = resolve(source)
  const resolvedTarget = resolve(targetDir)
  await verifyModelDirectory(resolvedSource)
  await rm(resolvedTarget, { recursive: true, force: true })
  await mkdir(resolvedTarget, { recursive: true })
  await Promise.all(
    MODEL_FILES.map((file) =>
      copyFile(join(resolvedSource, file.name), join(resolvedTarget, file.name)),
    ),
  )
  await verifyModelDirectory(resolvedTarget)
  await exportSpeechBpeVocab(join(resolvedTarget, 'bpe.model'), join(resolvedTarget, 'bpe.vocab'))
  return resolvedTarget
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [, , mode, sourceDir, targetDir] = process.argv
  if (mode !== '--source' || !sourceDir || !targetDir) {
    throw new Error(
      '用法：node scripts/stage-android-speech-model.mjs --source <模型目录> <assets/speech-model>',
    )
  }
  console.log(
    `Android speech model staged: ${await stageAndroidSpeechModel({ sourceDir, targetDir })}`,
  )
}
