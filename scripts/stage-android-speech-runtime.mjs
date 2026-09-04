import { createHash } from 'node:crypto'
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'

const VERSION = '1.13.7'
const ASSET_DOWNLOAD_URL =
  'https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.7/sherpa-onnx-1.13.7.aar'
const EXPECTED_SHA256 = 'c4ef49e309f24fcee5c106b8a279481aaecaabb078cd37b2cd6e9a62cc8a73c8'
const DOWNLOAD_ATTEMPTS = 3
const DOWNLOAD_TIMEOUT_MS = 600_000

async function sha256(path) {
  const digest = createHash('sha256')
  digest.update(await readFile(path))
  return digest.digest('hex')
}

async function downloadAar(cachePath) {
  for (let attempt = 1; attempt <= DOWNLOAD_ATTEMPTS; attempt += 1) {
    try {
      const response = await fetch(ASSET_DOWNLOAD_URL, {
        headers: { 'User-Agent': 'Pisper' },
        redirect: 'follow',
        signal: AbortSignal.timeout(DOWNLOAD_TIMEOUT_MS),
      })
      if (!response.ok || !response.body)
        throw new Error(`官方 Android sherpa Runtime 下载失败 (${response.status})。`)
      const bytes = Buffer.from(await response.arrayBuffer())
      const digest = createHash('sha256').update(bytes).digest('hex')
      if (digest !== EXPECTED_SHA256) throw new Error('Android sherpa Runtime SHA256 不匹配。')
      await writeFile(cachePath, bytes)
      return
    } catch (error) {
      if (attempt === DOWNLOAD_ATTEMPTS) throw error
      // 发布资源偶发连接重置，有限重试覆盖瞬时错误，固定摘要仍是最终信任边界。
      console.warn(`Android sherpa Runtime 下载失败，准备第 ${attempt + 1} 次尝试。`)
      await new Promise((resolveDelay) => setTimeout(resolveDelay, attempt * 1_000))
    }
  }
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
    // Release 直链不消耗 GitHub REST API 配额，CI 干净环境也能稳定下载。
    await mkdir(join(resolve(root), 'release', 'cache'), { recursive: true })
    await downloadAar(cachePath)
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
