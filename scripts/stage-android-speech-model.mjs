import { createHash } from 'node:crypto'
import { createReadStream } from 'node:fs'
import { access, copyFile, mkdir, rm } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const MODEL_FILES = [
  {
    name: 'model.int8.onnx',
    sha256: '68c9c943840f7d9cf3e8a4970ba50f404feb5277f611fa82b7e72267786fa84a',
  },
  {
    name: 'tokens.txt',
    sha256: '6fed8c6c248516f38e7faa19404b57413e8ce259f1cbc1fa4aebc86eac32fdfd',
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
