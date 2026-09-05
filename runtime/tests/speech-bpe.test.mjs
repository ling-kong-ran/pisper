import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'
import protobuf from 'protobufjs'
import { exportSpeechBpeVocab, sentencePieceModelToVocab } from '../../scripts/speech-bpe.mjs'
import { stageAndroidSpeechModel } from '../../scripts/stage-android-speech-model.mjs'
import { stageSpeechModel } from '../../scripts/stage-speech-model.mjs'

function encodePieces(pieces) {
  const writer = protobuf.Writer.create()
  for (const piece of pieces) {
    writer.uint32(10).fork()
    if (Object.hasOwn(piece, 'piece')) writer.uint32(10).string(piece.piece)
    if (Object.hasOwn(piece, 'score')) writer.uint32(21).float(piece.score)
    if (Object.hasOwn(piece, 'type')) writer.uint32(24).int32(piece.type)
    writer.ldelim()
  }
  // 未声明的训练配置必须像真实 SentencePiece 模型一样被跳过。
  writer.uint32(18).bytes(Buffer.from([8, 1]))
  return writer.finish()
}

const fixture = encodePieces([
  { piece: '<unk>', score: 0, type: 2 },
  { piece: '\u2581hello', score: -1.25, type: 1 },
  { piece: '\u4f60\u597d', score: -2.5, type: 1 },
  { piece: '<s>', type: 3 },
  { piece: '\ud83d\ude00', score: -3 },
])
const expected = '<unk>\t0\n\u2581hello\t-1.25\n\u4f60\u597d\t-2.5\n<s>\t0\n\ud83d\ude00\t-3\n'

async function temporaryDirectory(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-speech-bpe-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  return directory
}

function isolateModelSource(t) {
  const original = process.env.PISPER_SPEECH_MODEL_DIR
  delete process.env.PISPER_SPEECH_MODEL_DIR
  t.after(() => {
    if (original === undefined) delete process.env.PISPER_SPEECH_MODEL_DIR
    else process.env.PISPER_SPEECH_MODEL_DIR = original
  })
  t.mock.method(globalThis, 'fetch', () => {
    throw new Error('Tests must not download speech models.')
  })
}

async function writeModelFixture(directory) {
  await mkdir(directory, { recursive: true })
  for (const name of ['encoder.int8.onnx', 'decoder.onnx', 'joiner.int8.onnx', 'tokens.txt']) {
    await writeFile(join(directory, name), name)
  }
  await writeFile(join(directory, 'bpe.model'), fixture)
}

test('SentencePiece 词表保留词序、特殊词条、中文与分数并确定性输出 LF', () => {
  assert.equal(sentencePieceModelToVocab(fixture), expected)
  assert.equal(sentencePieceModelToVocab(Uint8Array.from(fixture)), expected)
  assert.equal(sentencePieceModelToVocab(fixture), sentencePieceModelToVocab(fixture))
})

test('拒绝非二进制、空或过大的模型以及异常词条数量', () => {
  assert.throws(() => sentencePieceModelToVocab('model'), /Uint8Array/)
  assert.throws(() => sentencePieceModelToVocab(Buffer.alloc(0)), /model size/)
  assert.throws(() => sentencePieceModelToVocab(Buffer.alloc(32 * 1024 * 1024 + 1)), /model size/)
  assert.throws(() => sentencePieceModelToVocab(encodePieces([])), /piece count/)
  const tooMany = Buffer.alloc(262_145 * 2)
  for (let index = 0; index < tooMany.length; index += 2) tooMany[index] = 10
  assert.throws(() => sentencePieceModelToVocab(tooMany), /piece count/)
})

test('拒绝损坏的 protobuf、非法 UTF-8、空白控制字符、重复词条及非有限分数', () => {
  assert.throws(() => sentencePieceModelToVocab(Buffer.from([10, 255])), /Invalid SentencePiece/)
  assert.throws(
    () => sentencePieceModelToVocab(Buffer.from([10, 3, 10, 1, 255])),
    /Invalid SentencePiece/,
  )
  for (const piece of ['', 'a\tb', 'a\nb', 'a\rb', 'a b', 'a\0b', '\u007f', 'a'.repeat(1025)]) {
    assert.throws(() => sentencePieceModelToVocab(encodePieces([{ piece }])), /Invalid BPE piece/)
  }
  assert.throws(() => sentencePieceModelToVocab(encodePieces([{}])), /Invalid BPE piece/)
  for (const score of [NaN, Infinity, -Infinity]) {
    assert.throws(
      () => sentencePieceModelToVocab(encodePieces([{ piece: 'hello', score }])),
      /Invalid BPE score/,
    )
  }
  assert.throws(
    () => sentencePieceModelToVocab(encodePieces([{ piece: 'hello' }, { piece: 'hello' }])),
    /Duplicate BPE piece/,
  )
})

test('文件导出覆盖旧词表且重复导出字节完全相同，非法模型不会覆盖输出', async (t) => {
  const directory = await temporaryDirectory(t)
  const modelPath = join(directory, 'bpe.model')
  const vocabPath = join(directory, 'bpe.vocab')
  await writeFile(modelPath, fixture)
  await writeFile(vocabPath, 'stale')
  assert.equal(await exportSpeechBpeVocab(modelPath, vocabPath), vocabPath)
  const first = await readFile(vocabPath)
  assert.deepEqual(first, Buffer.from(expected, 'utf8'))
  await exportSpeechBpeVocab(modelPath, vocabPath)
  assert.deepEqual(await readFile(vocabPath), first)
  await writeFile(modelPath, 'invalid')
  await assert.rejects(exportSpeechBpeVocab(modelPath, vocabPath), /Invalid SentencePiece/)
  assert.deepEqual(await readFile(vocabPath), first)
  await assert.rejects(exportSpeechBpeVocab(directory, vocabPath), /must be a file/)
})

test('Runtime staging 对环境来源与缓存来源都从五个上游文件重新生成词表', async (t) => {
  isolateModelSource(t)
  const root = await temporaryDirectory(t)
  const configuredSource = join(root, 'configured-model')
  const runtimeDir = join(root, 'staged-runtime')
  await writeModelFixture(configuredSource)
  await writeFile(join(configuredSource, 'bpe.vocab'), 'untrusted source vocab')
  process.env.PISPER_SPEECH_MODEL_DIR = configuredSource
  const target = await stageSpeechModel({ root, runtimeDir })
  assert.equal(await readFile(join(target, 'bpe.vocab'), 'utf8'), expected)

  delete process.env.PISPER_SPEECH_MODEL_DIR
  const cache = join(root, 'release', 'cache', 'speech-model')
  await writeModelFixture(cache)
  await writeFile(join(target, 'bpe.vocab'), 'stale target vocab')
  assert.equal(await stageSpeechModel({ root, runtimeDir }), target)
  assert.equal(await readFile(join(target, 'bpe.vocab'), 'utf8'), expected)
  await assert.rejects(readFile(join(cache, 'bpe.vocab')), /ENOENT/)

  await writeFile(join(cache, 'bpe.model'), 'invalid')
  await assert.rejects(stageSpeechModel({ root, runtimeDir }), /Invalid SentencePiece/)
})

test('Android staging 不接受只有文件名匹配的未验证模型', async (t) => {
  const root = await temporaryDirectory(t)
  const sourceDir = join(root, 'source')
  const targetDir = join(root, 'target')
  await writeModelFixture(sourceDir)
  await assert.rejects(stageAndroidSpeechModel({ sourceDir, targetDir }), /SHA256/)
  await assert.rejects(readFile(join(targetDir, 'bpe.vocab')), /ENOENT/)
})

test('可用的本地 x-asr 缓存通过 Android SHA 校验并导出一致词表（不联网）', async (t) => {
  const sourceDir = resolve('release/cache/speech-model')
  let model
  try {
    model = await readFile(join(sourceDir, 'bpe.model'))
  } catch (error) {
    if (error.code !== 'ENOENT') throw error
    t.skip('No local speech model cache.')
    return
  }
  if (
    createHash('sha256').update(model).digest('hex') !==
    'f87a38025a5fdd1e4e9591f6a44bb81295097ce0b80df6f4ab9f44e52c64ca5f'
  ) {
    t.skip('Local cache is not the pinned x-asr model.')
    return
  }
  const root = await temporaryDirectory(t)
  const targetDir = join(root, 'android-model')
  assert.equal(await stageAndroidSpeechModel({ sourceDir, targetDir }), targetDir)
  const first = await readFile(join(targetDir, 'bpe.vocab'))
  assert.deepEqual(first, Buffer.from(sentencePieceModelToVocab(model), 'utf8'))
  await exportSpeechBpeVocab(join(targetDir, 'bpe.model'), join(targetDir, 'bpe.vocab'))
  assert.deepEqual(await readFile(join(targetDir, 'bpe.vocab')), first)
  t.diagnostic(
    `Cached x-asr vocab: ${first.toString('utf8').split('\n').length - 1} pieces, ${first.length} bytes.`,
  )
})
