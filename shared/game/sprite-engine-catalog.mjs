// 资源必须由用户显式下载；版本、来源、字节数和摘要共同限定可执行 WASM/JS 的安装集合。
export class SpriteEngineError extends Error {
  /** @param {string} code @param {string} message @param {number} [statusCode] */
  constructor(code, message, statusCode = 400) {
    super(message)
    this.code = code
    this.statusCode = statusCode
  }
}
/** @type {readonly import('./sprite-engine-catalog.mjs').SpriteEngineDefinition[]} */
export const SPRITE_ENGINE_CATALOG = Object.freeze([
  {
    id: 'background',
    name: 'U²-Net small + ONNX Runtime Web',
    version: 'ort-1.20.1_u2netp-v0.0.0',
    licenses: [
      {
        name: 'ONNX Runtime — MIT',
        url: 'https://github.com/microsoft/onnxruntime/blob/v1.20.1/LICENSE',
      },
      {
        name: 'U²-Net — Apache-2.0',
        url: 'https://github.com/xuebinqin/U-2-Net/blob/ac7e1c817ecab7c7dff5ce6b1abba61cd213ff29/LICENSE',
      },
    ],
    files: [
      {
        name: 'ort.wasm.min.js',
        url: 'https://cdn.jsdelivr.net/npm/onnxruntime-web@1.20.1/dist/ort.wasm.min.js',
        bytes: 49026,
        sha256: 'e66568724f8848e57cc2a56e4bea3a5b86ce3ff81b2da11eac8ed7ab02b27bd4',
        mimeType: 'text/javascript',
      },
      {
        name: 'ort-wasm-simd-threaded.mjs',
        url: 'https://cdn.jsdelivr.net/npm/onnxruntime-web@1.20.1/dist/ort-wasm-simd-threaded.mjs',
        bytes: 24618,
        sha256: '745eb7c0ce6f18a6aa521971b2877babc7ffb27eecb58ab3bc6e5ef4692672e8',
        mimeType: 'text/javascript',
      },
      {
        name: 'ort-wasm-simd-threaded.wasm',
        url: 'https://cdn.jsdelivr.net/npm/onnxruntime-web@1.20.1/dist/ort-wasm-simd-threaded.wasm',
        bytes: 11246032,
        sha256: '207d02be4591c156b0a98f024f3d58005b5b04c92274d759fb390338c63559ea',
        mimeType: 'application/wasm',
      },
      {
        name: 'u2netp.onnx',
        url: 'https://gh-proxy.com/https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx',
        fallbackUrls: [
          'https://ghfast.top/https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx',
        ],
        bytes: 4574861,
        sha256: '309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8',
        mimeType: 'application/octet-stream',
      },
      {
        name: 'LICENSE-ORT.txt',
        url: 'https://cdn.jsdelivr.net/gh/microsoft/onnxruntime@v1.20.1/LICENSE',
        bytes: 1073,
        sha256: '2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c',
        mimeType: 'text/plain; charset=utf-8',
      },
      {
        name: 'LICENSE-U2NET.txt',
        url: 'https://cdn.jsdelivr.net/gh/xuebinqin/U-2-Net@ac7e1c817ecab7c7dff5ce6b1abba61cd213ff29/LICENSE',
        bytes: 11357,
        sha256: 'c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4',
        mimeType: 'text/plain; charset=utf-8',
      },
    ],
  },
  {
    id: 'inpaint',
    name: 'OpenCV.js inpainting',
    version: '5.0.0-release.1',
    licenses: [
      { name: 'OpenCV — Apache-2.0', url: 'https://github.com/opencv/opencv/blob/5.0.0/LICENSE' },
      {
        name: 'TechStark OpenCV.js — Apache-2.0',
        url: 'https://github.com/TechStark/opencv-js/blob/v5.0.0-release.1/LICENSE',
      },
    ],
    files: [
      {
        name: 'opencv.js',
        url: 'https://cdn.jsdelivr.net/npm/@techstark/opencv-js@5.0.0-release.1/dist/opencv.js',
        bytes: 13298869,
        sha256: 'b873c8211421da7b9bf41ae157a923f05a46a0b8d3e5904c44c6f3ad6d39a1bd',
        mimeType: 'text/javascript',
      },
      {
        name: 'LICENSE-OpenCV.txt',
        url: 'https://cdn.jsdelivr.net/gh/opencv/opencv@5.0.0/LICENSE',
        bytes: 11358,
        sha256: 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30',
        mimeType: 'text/plain; charset=utf-8',
      },
      {
        name: 'LICENSE-OpenCV-JS.txt',
        url: 'https://cdn.jsdelivr.net/gh/TechStark/opencv-js@v5.0.0-release.1/LICENSE',
        bytes: 11357,
        sha256: 'c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4',
        mimeType: 'text/plain; charset=utf-8',
      },
    ],
  },
])

/** @param {unknown} value @returns {import('./sprite-engine-catalog.mjs').SpriteEngineCatalog} */
export function parseSpriteEngineCatalog(value) {
  const invalid = () => new Error('Invalid sprite engine catalog')
  if (
    !value ||
    typeof value !== 'object' ||
    !('engines' in value) ||
    !Array.isArray(value.engines) ||
    value.engines.length > 2
  )
    throw invalid()
  const engines = value.engines.map((entry) => {
    if (!entry || typeof entry !== 'object') throw invalid()
    const id = entry.id
    const status = entry.status
    if (
      (id !== 'background' && id !== 'inpaint') ||
      !['missing', 'downloading', 'ready', 'failed'].includes(status)
    )
      throw invalid()
    for (const key of ['name', 'version', 'error'])
      if (typeof entry[key] !== 'string' || entry[key].length > 500) throw invalid()
    if (
      entry.file !== undefined &&
      (typeof entry.file !== 'string' || !/^[a-zA-Z0-9._-]{0,160}$/.test(entry.file))
    )
      throw invalid()
    for (const key of ['bytes', 'received', 'total'])
      if (!Number.isSafeInteger(entry[key]) || entry[key] < 0) throw invalid()
    if (entry.received > entry.total) throw invalid()
    return {
      id,
      name: entry.name,
      version: entry.version,
      bytes: entry.bytes,
      status,
      received: entry.received,
      total: entry.total,
      error: entry.error,
      file: entry.file ?? '',
    }
  })
  if (new Set(engines.map((engine) => engine.id)).size !== engines.length) throw invalid()
  return { engines }
}
