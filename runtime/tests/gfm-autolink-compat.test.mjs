// vendored mdast-util-gfm-autolink-literal 兼容层回归：
// 上游 email 自动链接规则内联 lookbehind 正则字面量，旧 iOS WKWebView（Safari <16.4）
// 解析即抛 "invalid group specifier name"，整条 markdown 渲染链（聊天/设置页）全挂。
// shim 用特性检测构造正则：现代引擎与原版行为零差异；旧引擎走无 lookbehind 降级分支，
// 边界校验由 findEmail 内既有的 previous() 逐字符兜底，两种引擎行为一致。
import assert from 'node:assert/strict'
import test from 'node:test'

const SHIM_PATH = '../../src/vendor/mdast-util-gfm-autolink-literal.js'

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

function paragraphTree(value) {
  return {
    type: 'root',
    children: [{ type: 'paragraph', children: [{ type: 'text', value }] }],
  }
}

// 对同一棵树跑 fromMarkdown 扩展的 transform，收集生成的链接节点。
async function linksFor(mod, text) {
  const tree = paragraphTree(text)
  const extension = mod.gfmAutolinkLiteralFromMarkdown()
  for (const transform of extension.transforms) transform(tree)
  const links = []
  const walk = (node) => {
    if (node.type === 'link') links.push(node)
    for (const child of node.children || []) walk(child)
  }
  walk(tree)
  return links
}

const CASES = [
  ['mail a@b.com end', 'mailto:a@b.com'],
  // 斜杠后的邮箱按 GFM 规则不自动链接（previous() 的 email 特判）。
  ['see x/y@c.com here', null],
  // 词内 @ 不构成边界：整段是普通文本时不应产生链接。
  ['nope notanemail', null],
]

test('modern engines keep the upstream lookbehind regex and its behavior', async () => {
  const mod = await import(`${SHIM_PATH}?modern`)
  for (const [text, expected] of CASES) {
    const links = await linksFor(mod, text)
    if (expected === null) assert.equal(links.length, 0, text)
    else assert.equal(links[0]?.url, expected, text)
  }
})

test('legacy engines fall back to an equivalent regex with identical results', async () => {
  globalThis.RegExp = LegacyJSCRegExp
  try {
    assert.throws(() => new RegExp('(?<=a)'), /invalid group specifier name/) // 环境自检：模拟生效
    const mod = await import(`${SHIM_PATH}?legacy`)
    for (const [text, expected] of CASES) {
      const links = await linksFor(mod, text)
      if (expected === null) assert.equal(links.length, 0, text)
      else assert.equal(links[0]?.url, expected, text)
    }
  } finally {
    globalThis.RegExp = NativeRegExp
  }
})
