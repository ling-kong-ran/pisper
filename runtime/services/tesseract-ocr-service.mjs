import { existsSync } from 'node:fs'
import { access, copyFile, mkdir } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import { tmpdir } from 'node:os'
import { fileURLToPath } from 'node:url'
import { createWorker } from 'tesseract.js'
import {
  OCR_LANGUAGES,
  OCR_MODEL_FILES,
  OCR_MODEL_VERSION,
} from '../../shared/ocr-model-catalog.mjs'

const require = createRequire(import.meta.url)
const WORKER_PATH = require.resolve('tesseract.js/src/worker-script/node/index.js')
const SUPPORTED_LANGUAGES = new Set(['eng', 'chi_sim', 'chi_sim+eng'])

function runtimeRoot() {
  const current = dirname(fileURLToPath(import.meta.url))
  return existsSync(join(current, 'ocr')) ? current : dirname(current)
}

function normalizeLanguage(value) {
  const language = String(value || 'chi_sim+eng')
    .trim()
    .toLowerCase()
  if (!SUPPORTED_LANGUAGES.has(language))
    throw new Error('OCR 仅支持 eng、chi_sim 或 chi_sim+eng。')
  return language
}

async function languagePath() {
  const directory = join(runtimeRoot(), 'ocr', OCR_MODEL_VERSION)
  try {
    await Promise.all(
      OCR_LANGUAGES.map((language) => access(join(directory, OCR_MODEL_FILES[language].file))),
    )
    return directory
  } catch {
    // 开发环境尚未执行 SEA staging 时，从 npm 已下载的语言包建立临时只读副本。
    const fallback = join(tmpdir(), 'pisper-ocr-models', OCR_MODEL_VERSION)
    await mkdir(fallback, { recursive: true })
    for (const language of OCR_LANGUAGES) {
      const model = OCR_MODEL_FILES[language]
      const target = join(fallback, model.file)
      try {
        await access(target)
      } catch {
        const source = require.resolve(`${model.package}/${OCR_MODEL_VERSION}/${model.file}`)
        await copyFile(source, target)
      }
    }
    return fallback
  }
}

export class TesseractOcrService {
  constructor() {
    this.workers = new Map()
    this.loading = new Map()
  }

  async getWorker(language) {
    const normalized = normalizeLanguage(language)
    const existing = this.workers.get(normalized)
    if (existing) return existing
    const pending = this.loading.get(normalized)
    if (pending) return pending
    const promise = (async () => {
      const worker = await createWorker(
        normalized.includes('+') ? normalized.split('+') : normalized,
        1,
        {
          workerPath: WORKER_PATH,
          langPath: await languagePath(),
          gzip: true,
          logger: () => {},
        },
      )
      this.workers.set(normalized, worker)
      return worker
    })()
    this.loading.set(normalized, promise)
    try {
      return await promise
    } finally {
      this.loading.delete(normalized)
    }
  }

  async recognize(input, { language = 'chi_sim+eng', signal } = {}) {
    if (signal?.aborted) throw new Error('OCR 操作已取消。')
    const worker = await this.getWorker(language)
    if (signal?.aborted) throw new Error('OCR 操作已取消。')
    const result = await worker.recognize(input)
    if (signal?.aborted) throw new Error('OCR 操作已取消。')
    return {
      text: String(result?.data?.text || ''),
      confidence: Number.isFinite(result?.data?.confidence) ? result.data.confidence : null,
      language: normalizeLanguage(language),
    }
  }

  async dispose() {
    const workers = [...this.workers.values()]
    this.workers.clear()
    this.loading.clear()
    await Promise.allSettled(workers.map((worker) => worker.terminate()))
  }
}

export function ocrRuntimeInfo() {
  return {
    languages: [...SUPPORTED_LANGUAGES],
    modelLanguages: [...OCR_LANGUAGES],
    modelVersion: OCR_MODEL_VERSION,
    modelFiles: Object.fromEntries(
      OCR_LANGUAGES.map((language) => [language, OCR_MODEL_FILES[language].file]),
    ),
  }
}

export function createTesseractOcrService() {
  return new TesseractOcrService()
}

export { normalizeLanguage }
