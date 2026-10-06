// OCR 模型目录由桌面 Runtime 与构建脚本共同使用，避免运行时和安装包使用不同语言或摘要。
export const OCR_LANGUAGES = Object.freeze(['eng', 'chi_sim'])
export const OCR_MODEL_VERSION = '4.0.0_best_int'
export const OCR_MODEL_FILES = Object.freeze({
  eng: {
    file: 'eng.traineddata.gz',
    package: '@tesseract.js-data/eng',
    sha256: '45b4cb346724ac1774f1c36f42f182b887bcdb28ebe63e6fff90ac41f3fcff91',
  },
  chi_sim: {
    file: 'chi_sim.traineddata.gz',
    package: '@tesseract.js-data/chi_sim',
    sha256: 'b8a23f10c7de500891eb458a8adc9cc58ab7f242f08b7d149f5e9aea4ad5db7c',
  },
})

export const OCR_MODEL_TOTAL_BYTES = 4_671_641
