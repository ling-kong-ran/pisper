import { readFile, stat, writeFile } from 'node:fs/promises'
import protobuf from 'protobufjs'

const MAX_MODEL_BYTES = 32 * 1024 * 1024
const MAX_PIECES = 262_144
const MAX_PIECE_BYTES = 1024
const modelType = protobuf
  .parse(
    `
    syntax = "proto2";
    message ModelProto {
      message SentencePiece {
        optional string piece = 1;
        optional float score = 2 [default = 0];
        optional int32 type = 3 [default = 1];
      }
      repeated SentencePiece pieces = 1;
    }
  `,
  )
  .root.lookupType('ModelProto')

function validateModelSize(size) {
  if (size < 1 || size > MAX_MODEL_BYTES) {
    throw new Error('BPE model size must be between 1 byte and 32 MiB.')
  }
}

export function sentencePieceModelToVocab(data) {
  if (!(data instanceof Uint8Array)) throw new TypeError('BPE model must be a Uint8Array.')
  validateModelSize(data.byteLength)
  const reader = protobuf.Reader.create(data)
  const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true })
  // protobufjs 默认容忍损坏的 UTF-8；构建时必须拒绝，避免热词词表悄悄改变。
  reader.string = function () {
    return decoder.decode(this.bytes())
  }
  let model
  try {
    model = modelType.decode(reader)
  } catch (cause) {
    throw new Error('Invalid SentencePiece BPE model.', { cause })
  }
  if (!model.pieces.length || model.pieces.length > MAX_PIECES) {
    throw new Error(`BPE piece count must be between 1 and ${MAX_PIECES}.`)
  }
  const seen = new Set()
  const lines = model.pieces.map(({ piece, score }, index) => {
    // 空白及控制字符会破坏 sherpa 的逐行词条/分数读取；SentencePiece 空格标记不受影响。
    if (
      !piece ||
      Buffer.byteLength(piece, 'utf8') > MAX_PIECE_BYTES ||
      /[\s\p{Cc}\p{Cs}]/u.test(piece)
    ) {
      throw new Error(`Invalid BPE piece at index ${index}.`)
    }
    if (!Number.isFinite(score)) throw new Error(`Invalid BPE score at index ${index}.`)
    if (seen.has(piece)) throw new Error(`Duplicate BPE piece at index ${index}.`)
    seen.add(piece)
    return `${piece}\t${score}\n`
  })
  return lines.join('')
}

export async function exportSpeechBpeVocab(modelPath, vocabPath) {
  const info = await stat(modelPath)
  if (!info.isFile()) throw new Error('BPE model must be a file.')
  validateModelSize(info.size)
  const vocab = sentencePieceModelToVocab(await readFile(modelPath))
  await writeFile(vocabPath, vocab, 'utf8')
  return vocabPath
}
