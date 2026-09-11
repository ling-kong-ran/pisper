import assert from 'node:assert/strict'
import { cp, mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'
import {
  hasStagedOcrModels,
  OCR_LANGUAGES,
  OCR_MODEL_FILES,
  OCR_MODEL_TOTAL_BYTES,
  OCR_MODEL_VERSION,
  stageOcrModels,
} from '../../scripts/stage-ocr-models.mjs'
import { normalizeLanguage, ocrRuntimeInfo } from '../services/tesseract-ocr-service.mjs'

const root = resolve(import.meta.dirname, '../..')

test('computer-use OCR keeps Chinese and English in one supported catalog', () => {
  assert.deepEqual(OCR_LANGUAGES, ['eng', 'chi_sim'])
  assert.deepEqual(ocrRuntimeInfo().languages, ['eng', 'chi_sim', 'chi_sim+eng'])
  assert.equal(normalizeLanguage('CHI_SIM+ENG'), 'chi_sim+eng')
  assert.throws(() => normalizeLanguage('jpn'), /仅支持 eng、chi_sim 或 chi_sim\+eng/)
  assert.equal(OCR_MODEL_TOTAL_BYTES < 20 * 1024 * 1024, true)
})

test('desktop staging copies and verifies both OCR models', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-ocr-stage-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  for (const packageName of ['tesseract.js', 'tesseract.js-core']) {
    await cp(
      join(root, 'node_modules', packageName),
      join(directory, 'node_modules', packageName),
      { recursive: true },
    )
  }
  for (const language of OCR_LANGUAGES) {
    const packageName = OCR_MODEL_FILES[language].package
    await cp(
      join(root, 'node_modules', ...packageName.split('/')),
      join(directory, 'node_modules', ...packageName.split('/')),
      { recursive: true },
    )
  }

  const staged = await stageOcrModels({ runtimeDir: directory, target: { platform: 'win32' } })
  assert.equal(staged, join(directory, 'runtime', 'ocr', OCR_MODEL_VERSION))
  assert.equal(await hasStagedOcrModels(directory), true)
  const model = await readFile(join(staged, OCR_MODEL_FILES.chi_sim.file))
  assert.ok(model.length > 1_000_000)
})

test('mobile staging does not copy desktop OCR resources', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mobile-ocr-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  assert.equal(
    await stageOcrModels({ runtimeDir: directory, target: { platform: 'mobile' } }),
    null,
  )
  assert.equal(await hasStagedOcrModels(directory), false)
})
