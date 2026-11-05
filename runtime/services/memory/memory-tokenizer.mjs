// 检索分词：FTS5 的 unicode61 会把一段连续汉字当成一个 token，存“深色主题”后搜“深色”查不到；
// trigram 分词器又查不到两字词。所以写入和查询两侧统一预分词（汉字单字 + 二元组、ASCII 词及其拆分），
// 用空格连起来再交给 FTS5。纯函数、零依赖；不走 ICU 词典，因为手机上的 Node 不一定带完整 ICU。
const CJK_CHAR = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/u
const WORD_RUN = /[\p{L}\p{N}_.\-/@]+/gu

// 查询侧的虚词：它们几乎不携带“在问什么”的信息，算进覆盖率只会稀释真正的关键词。
const STOPWORDS = new Set([
  // 中文二元组（含跨词切出来的常见碎片）
  '什么',
  '怎么',
  '怎样',
  '哪个',
  '哪些',
  '哪里',
  '这个',
  '那个',
  '这些',
  '那些',
  '我们',
  '你们',
  '他们',
  '是不',
  '不是',
  '可以',
  '应该',
  '需要',
  '一下',
  '一个',
  '的是',
  '是什',
  '么时',
  '时候',
  '有什',
  '了什',
  '么要',
  '行不',
  '不行',
  '在哪',
  '用什',
  '么方',
  '叫什',
  '吗',
  '呢',
  '吧',
  '的',
  '了',
  '是',
  '我',
  '你',
  '他',
  '它',
  '在',
  '有',
  '和',
  '与',
  '或',
  '也',
  '都',
  '就',
  '要',
  '会',
  '能',
  '把',
  '被',
  '给',
  '让',
  '用',
  '不',
  '么',
  '哪',
  '啥',
  '几',
  // 英文
  'the',
  'a',
  'an',
  'is',
  'are',
  'was',
  'were',
  'be',
  'do',
  'does',
  'did',
  'we',
  'you',
  'they',
  'how',
  'what',
  'which',
  'where',
  'when',
  'why',
  'who',
  'my',
  'your',
  'our',
  'i',
  'me',
  'to',
  'should',
  'can',
  'could',
  'would',
  'of',
  'in',
  'on',
  'at',
  'for',
  'and',
  'or',
  'it',
  'this',
  'that',
  'with',
  'from',
  's',
])

// 单字只有独立成词时才算关键词；二元组里已经有它，再算一遍会把高频字的噪声放大。
const isSingleCjk = (token) => Array.from(token).length === 1 && CJK_CHAR.test(token)

// 极轻的英文词形归一：deploy / deployment / deploys 落到同一个 token。
// 只为召回，两侧对称使用，所以不追求语言学上正确。
function lightStem(word) {
  if (!/^[a-z]+$/.test(word) || word.length <= 3) return word
  if (word.length > 7 && word.endsWith('ment')) return word.slice(0, -4)
  if (word.length > 5 && word.endsWith('ing')) return word.slice(0, -3)
  if (word.length > 4 && word.endsWith('ies')) return `${word.slice(0, -3)}y`
  if (word.length > 4 && word.endsWith('ed')) return word.slice(0, -2)
  if (word.length > 4 && word.endsWith('es')) return word.slice(0, -2)
  if (word.endsWith('s') && !word.endsWith('ss')) return word.slice(0, -1)
  return word
}

function asciiTokens(raw, out) {
  // 只切出字母数字：unicode61 本来也会在标点处断开，这里提前切好，查询两侧才对称。
  for (const part of raw.split(/[\s_.\-/@]+/)) {
    if (!part) continue
    const lower = part.toLowerCase()
    out.push(lightStem(lower))
    // AppSelect → appselect + app + select；safari16 → safari16 + safari
    const pieces = part
      .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
      .replace(/([a-zA-Z])(\d)/g, '$1 $2')
      .split(' ')
      .filter((piece) => piece.length > 1 && !/^\d+$/.test(piece))
    if (pieces.length > 1) for (const piece of pieces) out.push(lightStem(piece.toLowerCase()))
  }
}

function cjkTokens(run, out, withUnigrams) {
  const chars = Array.from(run)
  if (chars.length === 1) {
    out.push(chars[0])
    return
  }
  for (let index = 0; index < chars.length - 1; index += 1) {
    out.push(chars[index] + chars[index + 1])
  }
  if (withUnigrams) out.push(...chars)
}

// 把一段文字切成检索 token，保持出现顺序、去重。
// unigrams：索引侧为 true，单字查询（“猫”）才能命中“橘猫”；查询侧默认只用二元组。
export function searchTokens(text, { unigrams = false, dropStopwords = false } = {}) {
  const value = String(text || '').normalize('NFKC')
  const out = []
  for (const run of value.match(WORD_RUN) || []) {
    let ascii = ''
    let cjk = ''
    for (const char of run) {
      if (CJK_CHAR.test(char)) {
        if (ascii) asciiTokens(ascii, out)
        ascii = ''
        cjk += char
      } else {
        if (cjk) cjkTokens(cjk, out, unigrams)
        cjk = ''
        ascii += char
      }
    }
    if (ascii) asciiTokens(ascii, out)
    if (cjk) cjkTokens(cjk, out, unigrams)
  }
  const unique = [...new Set(out.filter(Boolean))]
  if (!dropStopwords) return unique
  const kept = unique.filter((token) => !STOPWORDS.has(token))
  // 全是虚词时（比如“是什么”）宁可保留原样，也不要让查询变空。
  return kept.length ? kept : unique
}

// 索引用的文本：空格分隔的 token，交给 FTS5 的 unicode61。
export const indexText = (text) => searchTokens(text, { unigrams: true }).join(' ')

export const queryTokens = (text) => searchTokens(text, { dropStopwords: true }).slice(0, 64)

// FTS5 MATCH 表达式：每个 token 都加引号，用户输入里的运算符不会被解释。
export function ftsExpression(tokens) {
  return tokens.map((token) => `"${token.replaceAll('"', '""')}"`).join(' OR ')
}

// 这段查询有没有足够的信息量。“好的”“继续”这类消息不值得检索，也最容易误命中。
export function isInformativeQuery(text) {
  const tokens = queryTokens(text)
  if (!tokens.length) return false
  if (tokens.some((token) => /^[a-z0-9]{3,}$/.test(token))) return true
  return tokens.filter((token) => !isSingleCjk(token)).length >= 2
}

const ENTITY_PATTERNS = [
  // 路径与带扩展名的文件
  /[\w.@-]+(?:\/[\w.@-]+)+/g,
  /\b[\w-]+\.(?:mjs|cjs|js|jsx|ts|tsx|json|md|py|go|rs|java|sql|sqlite|ya?ml|toml|sh|css|html)\b/g,
  // @scope/name
  /@[\w-]+\/[\w.-]+/g,
  // camelCase、PascalCase、snake_case、kebab-case、带数字的名字
  /\b[A-Za-z][a-z0-9]*(?:[A-Z_-][A-Za-z0-9]+)+\b/g,
  /\b[A-Za-z]+\d[\w.-]*\b/g,
  // 首字母大写的产品名（Vite、Neovim）与全大写缩写（CI、ECS）
  /\b[A-Z][a-z]{2,}\b/g,
  /\b[A-Z]{2,}\b/g,
]

// 规则抽取实体，写进倒排表：查询里出现同一个路径或包名时，这条比只是词面相近的更可信。
export function extractEntities(text) {
  const value = String(text || '').normalize('NFKC')
  const entities = new Set()
  for (const pattern of ENTITY_PATTERNS) {
    for (const match of value.matchAll(pattern)) {
      const entity = match[0].toLowerCase().replace(/[.\-/]+$/, '')
      if (entity.length >= 2 && entity.length <= 200) entities.add(entity)
    }
  }
  return [...entities].slice(0, 32)
}

// 查询里可能是实体的小写 token：覆盖“用户打的是小写，但记忆里写的是 Playwright”。
export function queryEntities(text) {
  const value = String(text || '')
    .normalize('NFKC')
    .toLowerCase()
  const candidates = new Set(extractEntities(text))
  for (const word of value.match(/[a-z0-9][\w.@/-]{1,}/g) || []) {
    const entity = word.replace(/[.\-/]+$/, '')
    if (entity.length >= 2 && !STOPWORDS.has(entity)) candidates.add(entity)
  }
  return [...candidates].slice(0, 32)
}

// MMR 去重用的相似度：只看 token 集合，不调模型。
export function jaccard(left, right) {
  if (!left.size || !right.size) return 0
  let shared = 0
  for (const token of left) if (right.has(token)) shared += 1
  return shared / (left.size + right.size - shared)
}
