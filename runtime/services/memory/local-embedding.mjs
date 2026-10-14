// 本地嵌入：无外部模型的离线向量化方案（哈希特征 + 归一化），
// 支撑记忆的语义检索；远程 embedding 不可用时作为降级路径。
const DEFAULT_DIMENSIONS = 384

// FNV-1a 哈希：把 token 稳定映射到向量下标。
function hashToken(token) {
  let hash = 2166136261
  for (const character of token) {
    hash ^= character.codePointAt(0)
    hash = Math.imul(hash, 16777619)
  }
  return hash >>> 0
}

function tokenize(value) {
  const normalized = String(value || '')
    .normalize('NFKC')
    .toLowerCase()
  const tokens = normalized.match(/[\p{L}\p{N}_-]+/gu) || []
  const features = []
  for (const token of tokens) {
    const characters = Array.from(token)
    if (characters.every((character) => character.codePointAt(0) <= 0x7f)) {
      features.push(token)
      for (let index = 0; index < characters.length - 2; index += 1)
        features.push(characters.slice(index, index + 3).join(''))
      continue
    }
    features.push(...characters)
    for (let index = 0; index < characters.length - 1; index += 1)
      features.push(characters.slice(index, index + 2).join(''))
    for (let index = 0; index < characters.length - 2; index += 1)
      features.push(characters.slice(index, index + 3).join(''))
  }
  return features
}

// 分词与特征抽取：ASCII 词取整词 + 3-gram；CJK 逐字 + 2/3-gram（中文分词近似），
// 保证同义/相似写法能产生重叠特征。
export function localEmbedding(value, dimensions = DEFAULT_DIMENSIONS) {
  const vector = new Float32Array(dimensions)
  for (const token of tokenize(value)) {
    const hash = hashToken(token)
    const index = hash % dimensions
    vector[index] += (hash & 0x80000000) === 0 ? 1 : -1
  }
  let magnitude = 0
  for (const number of vector) magnitude += number * number
  if (magnitude > 0) {
    const divisor = Math.sqrt(magnitude)
    for (let index = 0; index < vector.length; index += 1) vector[index] /= divisor
  }
  return vector
}

// 向量 → 二进制 Buffer（SQLite 存储用）。
export function embeddingBuffer(vector) {
  return Buffer.from(vector.buffer, vector.byteOffset, vector.byteLength)
}

// Buffer → 向量（拷贝还原，避免直接包一层可能被修改的底层 buffer）。
export function embeddingFromBuffer(buffer) {
  if (!buffer?.length) return new Float32Array()
  const bytes = Buffer.from(buffer)
  const copy = new Uint8Array(bytes.byteLength)
  copy.set(bytes)
  return new Float32Array(
    copy.buffer,
    0,
    Math.floor(copy.byteLength / Float32Array.BYTES_PER_ELEMENT),
  )
}

// 余弦相似度（归一化后等价于点积）。
export function cosineSimilarity(left, right) {
  const length = Math.min(left.length, right.length)
  let score = 0
  for (let index = 0; index < length; index += 1) score += left[index] * right[index]
  return score
}

// 关键词重叠率：查询与文本共享 token 的比例，混合检索时作为权重。
export function keywordOverlap(query, text) {
  const queryTokens = new Set(tokenize(query))
  if (!queryTokens.size) return 0
  const textTokens = new Set(tokenize(text))
  let matches = 0
  for (const token of queryTokens) if (textTokens.has(token)) matches += 1
  return matches / queryTokens.size
}
