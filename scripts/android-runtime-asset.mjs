// Android 安装包里的嵌入式 Runtime 改用 brotli（同一个 tar，比 gzip 小约三成）。
// 发布资产和 iOS 仍是签名过的 pisper-embedded-runtime.tar.gz；这里只把它转成 APK 资产。
// App 每次启动只读归档开头 16 KB 解析首项清单（embedded_runtime.rs 的 archive_manifest），
// 所以压完首项清单后先 flush 一次，让它落在单独的小数据块里，不必等整块 Huffman 表。
import { createHash } from 'node:crypto'
import { readFile, writeFile } from 'node:fs/promises'
import { constants, createBrotliCompress, gunzipSync, brotliDecompressSync } from 'node:zlib'

export const ANDROID_RUNTIME_ASSET = 'pisper-embedded-runtime.tar.br'
const TAR_BLOCK = 512

function firstEntryEnd(tar) {
  const size = Number.parseInt(
    tar.subarray(124, 136).toString('latin1').replace(/\0.*$/, '').trim(),
    8,
  )
  if (!Number.isFinite(size) || size < 0) throw new Error('Invalid embedded Runtime tar header')
  return TAR_BLOCK + Math.ceil(size / TAR_BLOCK) * TAR_BLOCK
}

export async function brotliTar(tar) {
  const encoder = createBrotliCompress({
    params: {
      [constants.BROTLI_PARAM_QUALITY]: 11,
      [constants.BROTLI_PARAM_LGWIN]: 24,
      [constants.BROTLI_PARAM_SIZE_HINT]: tar.length,
    },
  })
  const chunks = []
  encoder.on('data', (chunk) => chunks.push(chunk))
  const finished = new Promise((resolve, reject) => {
    encoder.on('end', resolve)
    encoder.on('error', reject)
  })
  const boundary = firstEntryEnd(tar)
  encoder.write(tar.subarray(0, boundary))
  await new Promise((resolve) => encoder.flush(constants.BROTLI_OPERATION_FLUSH, resolve))
  encoder.end(tar.subarray(boundary))
  await finished
  return Buffer.concat(chunks)
}

export async function writeAndroidRuntimeAsset(archivePath, outputPath) {
  const tar = gunzipSync(await readFile(archivePath))
  const compressed = await brotliTar(tar)
  await writeFile(outputPath, compressed)
  return { bytes: compressed.length, sha256: createHash('sha256').update(compressed).digest('hex') }
}

// APK 里的资产必须与签名过的 .tar.gz 是同一个 tar。
export async function verifyAndroidRuntimeAsset(archivePath, assetPath) {
  const expected = gunzipSync(await readFile(archivePath))
  const actual = brotliDecompressSync(await readFile(assetPath))
  if (!expected.equals(actual))
    throw new Error('Android Runtime asset differs from the signed archive')
}

if (import.meta.url === `file://${process.argv[1]?.replaceAll('\\', '/')}`) {
  const [mode, archive, asset] = process.argv.slice(2)
  if (mode === '--verify') {
    await verifyAndroidRuntimeAsset(archive, asset)
    console.log(`Android Runtime asset matches ${archive}`)
  } else {
    const { bytes } = await writeAndroidRuntimeAsset(mode, archive)
    console.log(`Android Runtime asset: ${archive} (${(bytes / 1024 / 1024).toFixed(1)} MiB)`)
  }
}
