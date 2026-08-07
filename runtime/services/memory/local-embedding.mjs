const DEFAULT_DIMENSIONS = 384

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

export function embeddingBuffer(vector) {
  return Buffer.from(vector.buffer, vector.byteOffset, vector.byteLength)
}

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

export function cosineSimilarity(left, right) {
  const length = Math.min(left.length, right.length)
  let score = 0
  for (let index = 0; index < length; index += 1) score += left[index] * right[index]
  return score
}

export function keywordOverlap(query, text) {
  const queryTokens = new Set(tokenize(query))
  if (!queryTokens.size) return 0
  const textTokens = new Set(tokenize(text))
  let matches = 0
  for (const token of queryTokens) if (textTokens.has(token)) matches += 1
  return matches / queryTokens.size
}
