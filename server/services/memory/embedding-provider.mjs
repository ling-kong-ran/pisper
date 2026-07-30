const DEFAULT_LSH_BANDS = 12
const DEFAULT_LSH_BITS = 8

export class EmbeddingProvider {
  constructor({ id, model, version, dimensions }) {
    this.id = String(id || '')
    this.model = String(model || '')
    this.version = String(version || '')
    this.dimensions = Math.max(1, Number(dimensions) || 1)
  }

  descriptor() {
    return {
      provider: this.id,
      model: this.model,
      version: this.version,
      dimensions: this.dimensions,
    }
  }

  async embed(_texts) {
    throw new Error('Embedding Provider 未实现 embed()。')
  }
}

function hyperplaneSign(dimension, band, bit) {
  let value = (dimension + 1) ^ Math.imul(band + 17, 0x45d9f3b) ^ Math.imul(bit + 31, 0x119de1f3)
  value = Math.imul(value ^ (value >>> 16), 0x45d9f3b)
  value = Math.imul(value ^ (value >>> 16), 0x45d9f3b)
  return (value ^ (value >>> 16)) < 0 ? -1 : 1
}

export function embeddingBuckets(vector, bands = DEFAULT_LSH_BANDS, bits = DEFAULT_LSH_BITS) {
  if (!vector?.length) return []
  const buckets = []
  for (let band = 0; band < bands; band += 1) {
    let signature = 0
    for (let bit = 0; bit < bits; bit += 1) {
      let projection = 0
      for (let dimension = 0; dimension < vector.length; dimension += 1) {
        projection += vector[dimension] * hyperplaneSign(dimension, band, bit)
      }
      if (projection >= 0) signature |= 1 << bit
    }
    buckets.push(`${band}:${signature.toString(16).padStart(Math.ceil(bits / 4), '0')}`)
  }
  return buckets
}

export function embeddingBuffer(vector) {
  if (!vector?.length) return null
  return Buffer.from(vector.buffer, vector.byteOffset, vector.byteLength)
}

export function embeddingFromBuffer(buffer) {
  if (!buffer?.length) return new Float32Array()
  const bytes = Buffer.from(buffer)
  const copy = new Uint8Array(bytes.byteLength)
  copy.set(bytes)
  return new Float32Array(copy.buffer, 0, Math.floor(copy.byteLength / Float32Array.BYTES_PER_ELEMENT))
}

export function cosineSimilarity(left, right) {
  if (!left?.length || left.length !== right?.length) return 0
  let score = 0
  for (let index = 0; index < left.length; index += 1) score += left[index] * right[index]
  return score
}
