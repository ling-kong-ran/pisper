// 前端产物语法兼容性审计：防止依赖升级悄悄引入旧 WebView 无法解析的语法。
//
// 背景：App 最低支持 iOS 15.1（src-tauri/tauri.mobile-ios.conf.json），而部分依赖
// （如 @radix-ui/react-collection）发布的产物含 class static block（Safari 16.4+ 才支持）。
// 这类语法进入初始 chunk 后，旧 iOS 的 WebView 会在模块解析阶段直接 SyntaxError，
// React 无法挂载，用户只能看到黑屏（2026-09 线上事故）。vite.config.ts 的 build.target
// 负责降级，本脚本是产物侧的最后闸门：任何人调高 target 或依赖引入新的解析期语法都会在这里失败。
//
// 检查项（均为 Safari 15.0 无法解析、且无法被字符串上下文简单排除的确定性语法）：
// 1. class static block：`static {` / `static{`（Safari 16.4+）
// 2. RegExp lookbehind 字面量：`/(?<=` `/(?<!`（Safari 16.4+；需排除字符串内的 Oniguruma 模式）
// 3. 私有字段 brand check：`#x in obj`（Safari 15.4+）
import { readdir, readFile } from 'node:fs/promises'
import { join } from 'node:path'

const assetsDir = join(process.cwd(), 'dist', 'assets')

/** 去掉字符串与模板字面量后再匹配，避免把 Oniguruma 模式等字符串内容误报为语法。 */
function stripStringsAndComments(source) {
  let out = ''
  let i = 0
  const n = source.length
  while (i < n) {
    const ch = source[i]
    if (ch === '"' || ch === "'" || ch === '`') {
      // 跳过整个字符串（处理转义与模板里的 ${} 嵌套——嵌套表达式按普通字符跳过即可，
      // 因为表达式内若再出现字符串会继续被本循环处理；这里只需保证不误判引号配对）。
      const quote = ch
      i += 1
      while (i < n) {
        if (source[i] === '\\') {
          i += 2
          continue
        }
        if (source[i] === quote) {
          i += 1
          break
        }
        i += 1
      }
      out += ' '
      continue
    }
    if (ch === '/' && source[i + 1] === '/') {
      while (i < n && source[i] !== '\n') i += 1
      continue
    }
    if (ch === '/' && source[i + 1] === '*') {
      i += 2
      while (i < n && !(source[i] === '*' && source[i + 1] === '/')) i += 1
      i += 2
      continue
    }
    out += ch
    i += 1
  }
  return out
}

const CHECKS = [
  {
    name: 'class static block (Safari 16.4+)',
    // class 体内的 static 初始化块；排除 `static x = ...` 等带名字的成员。
    pattern: /static\s*\{/g,
    onStripped: true,
  },
  {
    name: 'RegExp lookbehind 字面量 (Safari 16.4+)',
    // 正则字面量开头的 (?<= / (?<!；前面不能是标识符/右括号（那会是除法或字符串残留）。
    // 在去掉字符串后的源码上查：shiki 语法的 Onigurama 模式是字符串，必须排除；
    // 真·正则字面量不含引号，strip 后会原样保留。
    pattern: /(?:^|[^)\]}'"A-Za-z0-9_$.])\/\(\?<[=!]/g,
    onStripped: true,
  },
  {
    name: '私有字段 brand check (Safari 15.4+)',
    pattern: /#[A-Za-z_$][A-Za-z0-9_$]*\s+in\s+(?:[A-Za-z_$(]|\bthis\b)/g,
    onStripped: true,
  },
]

let files
try {
  files = (await readdir(assetsDir)).filter((name) => name.endsWith('.js'))
} catch {
  console.error('dist/assets 不存在，请先运行 npm run build。')
  process.exit(1)
}

const failures = []
for (const file of files.sort()) {
  const source = await readFile(join(assetsDir, file), 'utf8')
  const stripped = stripStringsAndComments(source)
  for (const check of CHECKS) {
    const haystack = check.onStripped ? stripped : source
    check.pattern.lastIndex = 0
    const match = check.pattern.exec(haystack)
    if (match) {
      const at = Math.max(0, match.index - 40)
      failures.push(
        `${file}: ${check.name} @${match.index}\n    …${haystack.slice(at, match.index + 60).replace(/\n/g, ' ')}…`,
      )
    }
  }
}

if (failures.length) {
  console.error(`前端产物含有旧 WebView 无法解析的语法（共 ${failures.length} 处）：`)
  for (const item of failures) console.error(`  - ${item}`)
  console.error(
    '\n处理：确认 vite.config.ts 的 build.target 仍为 safari16（不得高于 iOS 最低支持版本对应的 Safari），' +
      '并检查新引入/升级的依赖是否发布了更高版本的语法产物。',
  )
  process.exit(1)
}

console.log(`前端产物语法兼容性审计通过（${files.length} 个 JS，Safari 15 解析安全）。`)
