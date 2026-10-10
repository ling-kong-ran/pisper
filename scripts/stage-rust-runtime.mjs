import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { stageRustSpeech } from './stage-rust-speech.mjs'

// 桌面与独立 CLI 共用资源清单和许可证，防止两种原生产物发生漂移。
export async function stageRustRuntime({
  root,
  runtimeDir,
  runtimeExecutable,
  runtimeVersion,
  sourceCommit,
  sourceDirty,
}) {
  await mkdir(runtimeDir, { recursive: true })
  const speechResources = await stageRustSpeech({ root, targetDir: runtimeDir })
  const teleaSource = await readFile(
    path.join(root, 'runtime-rs/src/native_image_runtime/telea.rs'),
    'utf8',
  )
  const teleaLicense = teleaSource.match(/\/\*\r?\n(Intel License Agreement[\s\S]*?)\r?\n\*\//)?.[1]
  if (!teleaLicense) throw new Error('Native Telea license must accompany binary distributions.')
  await mkdir(path.join(runtimeDir, 'licenses'), { recursive: true })
  await writeFile(path.join(runtimeDir, 'licenses/OPENCV-TELEA.txt'), `${teleaLicense}\n`)
  await mkdir(runtimeDir, { recursive: true })
  await writeFile(
    path.join(runtimeDir, 'package.json'),
    `${JSON.stringify(
      {
        name: 'pisper-runtime',
        version: runtimeVersion,
        backend: 'rust',
        sourceCommit,
        sourceDirty,
        executableSha256: createHash('sha256')
          .update(await readFile(runtimeExecutable))
          .digest('hex'),
        speechResources,
        nativeImageLicense: {
          path: 'licenses/OPENCV-TELEA.txt',
          source: 'https://github.com/opencv/opencv/blob/5.0.0/modules/photo/src/inpaint.cpp',
          sha256: createHash('sha256').update(`${teleaLicense}\n`).digest('hex'),
        },
      },
      null,
      2,
    )}\n`,
  )
}
