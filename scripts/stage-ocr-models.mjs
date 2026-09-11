import { createHash } from 'node:crypto'
import { access, copyFile, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'

import {
  OCR_LANGUAGES,
  OCR_MODEL_FILES,
  OCR_MODEL_VERSION,
  OCR_MODEL_TOTAL_BYTES,
} from '../shared/ocr-model-catalog.mjs'

async function sha256File(filePath) {
  const hash = createHash('sha256')
  hash.update(await readFile(filePath))
  return hash.digest('hex')
}

async function trimTesseractCore(runtimeDir) {
  const coreDir = join(runtimeDir, 'node_modules', 'tesseract.js-core')
  const entries = await readdir(coreDir)
  const keep = new Set([
    'package.json',
    'index.js',
    'tesseract-core-lstm.js',
    'tesseract-core-lstm.wasm',
  ])
  await Promise.all(
    entries
      .filter((entry) => !keep.has(entry))
      .map((entry) => rm(join(coreDir, entry), { recursive: true, force: true })),
  )
  // 固定使用通用 LSTM Core，避免为不同 SIMD 能力重复携带数十 MB 的 wasm 变体。
  await writeFile(
    join(runtimeDir, 'node_modules', 'tesseract.js', 'src', 'worker-script', 'node', 'getCore.js'),
    "module.exports = async () => require('tesseract.js-core/tesseract-core-lstm')\n",
    'utf8',
  )
}

async function copyVerifiedModel(source, target, expectedSha256) {
  await access(source)
  const actualSha256 = await sha256File(source)
  if (actualSha256 !== expectedSha256) {
    throw new Error(`OCR 模型校验失败：${source}`)
  }
  await copyFile(source, target)
}

/**
 * 将 npm 安装阶段自动下载的 Tesseract 语言包放进 Runtime 闭包。
 * 不在运行时联网下载，避免首次 OCR 受网络、权限和 CDN 状态影响。
 */
export async function stageOcrModels({ runtimeDir, target }) {
  if (target?.platform === 'mobile') return null
  const sourceRoot = resolve(runtimeDir)
  const targetDir = join(sourceRoot, 'runtime', 'ocr', OCR_MODEL_VERSION)
  await rm(join(sourceRoot, 'runtime', 'ocr'), { recursive: true, force: true })
  await mkdir(targetDir, { recursive: true })

  for (const language of OCR_LANGUAGES) {
    const model = OCR_MODEL_FILES[language]
    const source = join(
      sourceRoot,
      'node_modules',
      ...model.package.split('/'),
      OCR_MODEL_VERSION,
      model.file,
    )
    await copyVerifiedModel(source, join(targetDir, model.file), model.sha256)
  }
  await trimTesseractCore(sourceRoot)
  return targetDir
}

export function ocrModelPath(runtimeDir, language) {
  const model = OCR_MODEL_FILES[language]
  if (!model) throw new Error(`不支持的 OCR 语言：${language}`)
  return join(resolve(runtimeDir), 'runtime', 'ocr', OCR_MODEL_VERSION, model.file)
}

export { OCR_LANGUAGES, OCR_MODEL_FILES, OCR_MODEL_VERSION, OCR_MODEL_TOTAL_BYTES }
// 供构建脚本做显式的模型存在性检查，也方便测试不启动 OCR Worker。
export async function hasStagedOcrModels(runtimeDir) {
  try {
    await Promise.all(
      OCR_LANGUAGES.map(async (language) => {
        const path = ocrModelPath(runtimeDir, language)
        await access(path)
        if ((await sha256File(path)) !== OCR_MODEL_FILES[language].sha256)
          throw new Error(`OCR 模型校验失败：${path}`)
      }),
    )
    return true
  } catch {
    return false
  }
}

export { sha256File }
