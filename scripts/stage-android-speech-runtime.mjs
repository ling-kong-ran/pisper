import { createHash } from 'node:crypto'
import { access, copyFile, cp, mkdir, readFile, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'

const VERSION = '1.13.7'
const ASSET_API_URL = 'https://api.github.com/repos/k2-fsa/sherpa-onnx/releases/assets/539211387'
const EXPECTED_SHA256 = 'c4ef49e309f24fcee5c106b8a279481aaecaabb078cd37b2cd6e9a62cc8a73c8'

async function sha256(path) {
  const digest = createHash('sha256')
  digest.update(await readFile(path))
  return digest.digest('hex')
}

export async function stageAndroidSpeechModel({ targetDir }) {
  const source = String(process.env.PISPER_SPEECH_MODEL_DIR || '').trim()
  if (!source) throw new Error('Android 构建需要 PISPER_SPEECH_MODEL_DIR 指向已下载的语音模型。')
  for (const name of ['model.int8.onnx', 'tokens.txt']) {
    await access(join(resolve(source), name)).catch(() => {
      throw new Error(`语音模型缺少 ${name}。`)
    })
  }
  await mkdir(targetDir, { recursive: true })
  await Promise.all(
    ['model.int8.onnx', 'tokens.txt'].map((name) =>
      cp(join(resolve(source), name), join(targetDir, name)),
    ),
  )
}

export async function stageAndroidSpeechRuntime({ root }) {
  const cachePath = join(resolve(root), 'release', 'cache', `sherpa-onnx-${VERSION}.aar`)
  const target = join(
    resolve(root),
    'src-tauri',
    'mobile-device-plugin',
    'android',
    'libs',
    'sherpa-onnx.aar',
  )
  let valid = false
  try {
    valid = (await sha256(cachePath)) === EXPECTED_SHA256
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }
  if (!valid) {
    const response = await fetch(ASSET_API_URL, {
      headers: { Accept: 'application/octet-stream', 'User-Agent': 'Pisper' },
    })
    if (!response.ok || !response.body)
      throw new Error(`官方 Android sherpa Runtime 下载失败 (${response.status})。`)
    await mkdir(join(resolve(root), 'release', 'cache'), { recursive: true })
    await writeFile(cachePath, Buffer.from(await response.arrayBuffer()))
    if ((await sha256(cachePath)) !== EXPECTED_SHA256)
      throw new Error('Android sherpa Runtime SHA256 不匹配。')
  }
  await mkdir(join(resolve(root), 'src-tauri', 'mobile-device-plugin', 'android', 'libs'), {
    recursive: true,
  })
  await copyFile(cachePath, target)
  return target
}

if (import.meta.url === `file://${process.argv[1]?.replaceAll('\\', '/')}`) {
  console.log(
    `Android sherpa Runtime staged: ${await stageAndroidSpeechRuntime({ root: process.cwd() })}`,
  )
}
