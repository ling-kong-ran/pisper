// GitHub 发布的 APK 不带语音原生库（onnxruntime + sherpa JNI），改为第一次下载语音模型时
// 从同一个 App Release 下载（SpeechNativeRuntime.kt）。这里打包下载用的压缩包，并把它的
// 大小、摘要、地址写进 APK 资产 speech-native-android.json；摘要随 APK 签名一起受保护。
// 商店包不能下载可执行代码，本地构建也没有对应的 Release，二者都保持随包安装（bundled）。
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { copyFile, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { create as createTar } from 'tar'
import { SHERPA_NATIVE_LIBRARIES } from './sherpa-aar.mjs'
import {
  ANDROID_SPEECH_NATIVE_VERSION,
  androidSpeechNativeDir,
} from './stage-android-speech-runtime.mjs'

export const ANDROID_SPEECH_NATIVE_MANIFEST = 'speech-native-android.json'
export const ANDROID_SPEECH_NATIVE_ARCHIVE = `pisper-speech-native-android-arm64-v8a-${ANDROID_SPEECH_NATIVE_VERSION}.tar.bz2`
const PREFIX = 'speech-native'

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex')
}

async function bzip2(input, output) {
  const child = spawn('bzip2', ['-9', '-c'], { stdio: ['pipe', 'pipe', 'inherit'] })
  const chunks = []
  child.stdout.on('data', (chunk) => chunks.push(chunk))
  const exited = new Promise((resolveExit, reject) => {
    child.on('error', reject)
    child.on('close', (code) =>
      code === 0 ? resolveExit() : reject(new Error(`bzip2 exited with ${code}`)),
    )
  })
  child.stdin.end(input)
  await exited
  await writeFile(output, Buffer.concat(chunks))
}

export function androidSpeechNativeMode(env = process.env) {
  const mode = String(env.PISPER_ANDROID_SPEECH_NATIVE || 'bundled').trim()
  if (!['bundled', 'download'].includes(mode)) {
    throw new Error(`PISPER_ANDROID_SPEECH_NATIVE must be bundled or download, got ${mode}`)
  }
  return mode
}

export async function stageAndroidSpeechNative({
  root,
  assetsDir,
  mode,
  version,
  repository = 'ling-kong-ran/pisper',
}) {
  root = resolve(root)
  const manifestPath = join(assetsDir, ANDROID_SPEECH_NATIVE_MANIFEST)
  if (mode === 'bundled') {
    // 换回随包安装时清掉上次留下的清单，否则 App 会去下载已经装在 APK 里的库。
    await rm(manifestPath, { force: true })
    return null
  }
  if (!/^\d+\.\d+\.\d+$/.test(String(version || ''))) {
    throw new Error(`Android speech native download needs the App version, got ${version}`)
  }
  const source = androidSpeechNativeDir(root)
  const staging = await mkdtemp(join(tmpdir(), 'pisper-speech-native-'))
  try {
    await mkdir(join(staging, PREFIX))
    const files = []
    for (const name of SHERPA_NATIVE_LIBRARIES) {
      const bytes = await readFile(join(source, name))
      await copyFile(join(source, name), join(staging, PREFIX, name))
      files.push({ path: name, bytes: bytes.length, sha256: sha256(bytes) })
    }
    const chunks = []
    for await (const chunk of createTar(
      { cwd: staging, portable: true, noMtime: true, noDirRecurse: true },
      [PREFIX, ...SHERPA_NATIVE_LIBRARIES.map((name) => `${PREFIX}/${name}`)],
    ))
      chunks.push(chunk)
    const archive = join(root, 'release', ANDROID_SPEECH_NATIVE_ARCHIVE)
    await mkdir(join(root, 'release'), { recursive: true })
    await bzip2(Buffer.concat(chunks), archive)
    const archiveBytes = await readFile(archive)
    const download = `https://github.com/${repository}/releases/download/app-v${version}/${ANDROID_SPEECH_NATIVE_ARCHIVE}`
    const manifest = {
      version: 1,
      id: `sherpa-onnx-${ANDROID_SPEECH_NATIVE_VERSION}-arm64-v8a`,
      archive: {
        format: 'tar.bz2',
        bytes: (await stat(archive)).size,
        sha256: sha256(archiveBytes),
        // 与语音模型目录一致：镜像在前，GitHub 原地址兜底。
        urls: [`https://ghfast.top/${download}`, download],
        stripPrefix: `${PREFIX}/`,
      },
      files,
    }
    await mkdir(assetsDir, { recursive: true })
    await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`)
    return { archive, manifest }
  } finally {
    await rm(staging, { recursive: true, force: true })
  }
}
