// 旧 Safari（<16.4）没有 lookbehind：formatSpeechTerms 必须降级为等价正则，
// 且热词归一化行为与现代引擎完全一致（断言逐条对齐 speech-hotword-integration）。
// 本文件需独立进程运行（node --test 每文件一进程）：文件顶部先把 globalThis.RegExp
// 换成「遇 lookbehind 抛 SyntaxError」的旧 JSC 模拟，再导入被测模块，
// 使 supportsLookbehind() 的首次探测落入 catch 分支。
import assert from 'node:assert/strict'
import test from 'node:test'
import { BUILTIN_SPEECH_TERMS } from '../services/speech-terms-service.mjs'

const NativeRegExp = globalThis.RegExp
const LOOKBEHIND = /\(\?<[=!]/

class LegacyJSCRegExp extends NativeRegExp {
  constructor(pattern, flags) {
    const source =
      typeof pattern === 'string' ? pattern : pattern instanceof NativeRegExp ? pattern.source : ''
    if (LOOKBEHIND.test(source)) {
      throw new SyntaxError('Invalid regular expression: invalid group specifier name')
    }
    super(pattern, flags)
  }
}

globalThis.RegExp = LegacyJSCRegExp

// 探测在首次调用时进行，此时全局 RegExp 已被替换，模块必然走降级分支。
const { formatSpeechTerms } = await import('../../shared/speech-terms.mjs')

test('patched RegExp rejects lookbehind so the legacy branch is exercised', () => {
  assert.throws(() => new RegExp('(?<=a)'), /invalid group specifier name/)
  assert.doesNotThrow(() => new RegExp('(?!a)'))
})

test('hotword normalization on the fallback branch matches the modern engine', () => {
  const terms = [...BUILTIN_SPEECH_TERMS, 'Py', 'Pi', 'projectFile']
  assert.equal(
    formatSpeechTerms('use effect, TYPE SCRIPT; javascript and react', terms),
    'useEffect, TypeScript; JavaScript and React',
  )
  assert.equal(formatSpeechTerms('use\t effect and pi agent', terms), 'useEffect and Pi Agent')
  assert.equal(formatSpeechTerms('py pi python', terms), 'py pi Python')
  const paths =
    'py/python .py main.py /tmp/python/useeffect.ts ./useeffect ../react C:\\python\\useeffect.ts python.py react-component x_useeffect myuseeffect projectFile.js'
  assert.equal(formatSpeechTerms(paths, terms), paths)
  assert.equal(
    formatSpeechTerms('happy copy pyproject pythonic reactivate', terms),
    'happy copy pyproject pythonic reactivate',
  )
  assert.equal(formatSpeechTerms('use effect and python', []), 'use effect and python')
  assert.equal(formatSpeechTerms('cargo test and npm install', terms), 'cargo test and npm install')
})

test('sentence periods stay punctuation and file extensions survive on the fallback', () => {
  const terms = ['useEffect', 'TypeScript']
  assert.equal(
    formatSpeechTerms('Use effect. Then type script.', terms),
    'useEffect. Then TypeScript.',
  )
  assert.equal(
    formatSpeechTerms('useeffect.py typescript.ts ./useeffect', terms),
    'useeffect.py typescript.ts ./useeffect',
  )
})
