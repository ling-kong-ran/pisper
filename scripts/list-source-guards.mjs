import { existsSync } from 'node:fs'
import { readdir, readFile } from 'node:fs/promises'
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

// 仓库根目录：脚本位于 scripts/ 下，向上一级即根目录，避免依赖运行时 cwd。
const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')

// 源码守卫测试集中在 runtime/tests/ 下，通过 readFile 读取源码并做正则断言。
const TESTS_DIRECTORY = 'runtime/tests'

// 读取测试文件自身的调用，识别下列静态可解析的形式：
//   1. readFile('路径') / readFileSync('路径')
//   2. readFile(resolve(root, '路径')) / readFile(join(root, '路径'))
//   3. readFile(new URL('路径', ROOT)) / readFile(new URL('路径', import.meta.url))
// 变量拼接（如 join(dataDir, name)、[...].map(readFile)）无法静态确定，直接忽略。
const READ_CALL_PATTERN = /\breadFile(?:Sync)?\s*\(/g

function skipWhitespace(source, index) {
  let cursor = index
  while (cursor < source.length && /\s/.test(source[cursor])) cursor += 1
  return cursor
}

// 从引号处扫描完整字符串字面量，返回字面量内容与结束位置。
function scanStringLiteral(source, start) {
  const quote = source[start]
  let cursor = start + 1
  while (cursor < source.length) {
    const char = source[cursor]
    if (char === '\\') {
      cursor += 2
      continue
    }
    if (char === quote) return { value: source.slice(start + 1, cursor), next: cursor + 1 }
    cursor += 1
  }
  return null
}

// 在括号内寻找顶层逗号，忽略嵌套括号与字符串内部的逗号。
function findTopLevelComma(source, start) {
  let depth = 0
  let cursor = start
  while (cursor < source.length) {
    const char = source[cursor]
    if (char === "'" || char === '"' || char === '`') {
      const literal = scanStringLiteral(source, cursor)
      if (!literal) return -1
      cursor = literal.next
      continue
    }
    if (char === '(') depth += 1
    if (char === ')') {
      if (depth === 0) return -1
      depth -= 1
    }
    if (char === ',' && depth === 0) return cursor
    cursor += 1
  }
  return -1
}

// 解析 readFile 调用的第一个实参，仅接受能静态还原出路径的形式。
function parseFirstArgument(source, index) {
  const start = skipWhitespace(source, index)
  const char = source[start]

  if (char === "'" || char === '"') {
    const literal = scanStringLiteral(source, start)
    return literal ? { kind: 'plain', value: literal.value } : null
  }

  // 模板字符串只有在不含插值时才是静态路径。
  if (char === '`') {
    const literal = scanStringLiteral(source, start)
    if (!literal || literal.value.includes('${')) return null
    return { kind: 'plain', value: literal.value }
  }

  const rest = source.slice(start, start + 48)

  const wrapper = /^(resolve|join)\s*\(/.exec(rest)
  if (wrapper) {
    // resolve(root, '路径') 形式：第二个实参必须是字符串字面量。
    const comma = findTopLevelComma(source, start + wrapper[0].length)
    if (comma < 0) return null
    const literalStart = skipWhitespace(source, comma + 1)
    const quote = source[literalStart]
    if (quote !== "'" && quote !== '"' && quote !== '`') return null
    const literal = scanStringLiteral(source, literalStart)
    if (!literal) return null
    const close = skipWhitespace(source, literal.next)
    if (source[close] !== ')') return null
    return { kind: 'joined', value: literal.value }
  }

  const urlCall = /^new\s+URL\s*\(/.exec(rest)
  if (urlCall) {
    // new URL('路径', 基准) 形式：基准决定路径的解析起点。
    const literalStart = skipWhitespace(source, start + urlCall[0].length)
    const quote = source[literalStart]
    if (quote !== "'" && quote !== '"' && quote !== '`') return null
    const literal = scanStringLiteral(source, literalStart)
    if (!literal) return null
    const comma = skipWhitespace(source, literal.next)
    if (source[comma] !== ',') return null
    const baseStart = skipWhitespace(source, comma + 1)
    let depth = 0
    let cursor = baseStart
    while (cursor < source.length) {
      const current = source[cursor]
      if (current === '(') depth += 1
      if (current === ')') {
        if (depth === 0) break
        depth -= 1
      }
      cursor += 1
    }
    const base = source.slice(baseStart, cursor).trim()
    // import.meta.url 相对测试文件自身；裸标识符（ROOT/root）在本仓库约定为仓库根目录。
    if (base === 'import.meta.url') return { kind: 'meta-url', value: literal.value }
    if (/^[A-Za-z_$][\w$]*$/.test(base)) return { kind: 'root-url', value: literal.value }
    return null
  }

  return null
}

// 把候选路径归一化为仓库根目录下的相对路径；不存在或越界的路径返回 null。
function normalizeCandidate(candidate, rootDirectory, testDirectory) {
  const base = candidate.kind === 'meta-url' ? testDirectory : rootDirectory
  const resolved = resolve(base, candidate.value)
  const rel = relative(rootDirectory, resolved)
  if (!rel || rel.startsWith('..') || isAbsolute(rel)) return null
  const relativePath = rel.split(sep).join('/')
  // 只统计真实存在的文件；顺带排除临时目录等运行时才出现的路径。
  if (!existsSync(resolved)) return null
  // 测试目录下的夹具不是被守卫的源码，跳过以避免误导重构排查。
  if (relativePath === TESTS_DIRECTORY || relativePath.startsWith(`${TESTS_DIRECTORY}/`)) {
    return null
  }
  return relativePath
}

function extractGuardCandidates(source, rootDirectory, testDirectory) {
  const candidates = new Set()
  for (const match of source.matchAll(READ_CALL_PATTERN)) {
    const parsed = parseFirstArgument(source, match.index + match[0].length)
    if (!parsed) continue
    const normalized = normalizeCandidate(parsed, rootDirectory, testDirectory)
    if (normalized) candidates.add(normalized)
  }
  return [...candidates].sort()
}

// 构建「测试文件 → 源文件集合」映射，键为 runtime/tests 下的相对测试路径。
export async function collectSourceGuards(rootDirectory = REPO_ROOT) {
  const testsDirectory = resolve(rootDirectory, TESTS_DIRECTORY)
  const entries = await readdir(testsDirectory)
  const guards = new Map()
  for (const name of entries.filter((file) => file.endsWith('.test.mjs')).sort()) {
    const testPath = `${TESTS_DIRECTORY}/${name}`
    const source = await readFile(resolve(testsDirectory, name), 'utf8')
    const candidates = extractGuardCandidates(source, rootDirectory, testsDirectory)
    if (candidates.length) guards.set(testPath, candidates)
  }
  return guards
}

// 反查：某个源文件被哪些测试文件守卫。
export function findTestsGuardingSource(guards, sourcePath) {
  const normalized = sourcePath.split('\\').join('/').replace(/^\.\//, '')
  const tests = []
  for (const [testFile, sources] of guards) {
    if (sources.includes(normalized)) tests.push(testFile)
  }
  return tests.sort()
}

export async function runCli(argv, rootDirectory = REPO_ROOT) {
  const args = argv.slice(2)
  if (args[0] === '--help' || args[0] === '-h') {
    console.log('Usage: node scripts/list-source-guards.mjs [--source <path>]')
    return 0
  }

  const guards = await collectSourceGuards(rootDirectory)

  if (args[0] === '--source') {
    if (!args[1]) {
      console.error('--source requires a path, e.g. --source src/features/chat/FocusSession.tsx')
      return 2
    }
    const input = args[1]
    const sourcePath = isAbsolute(input)
      ? relative(rootDirectory, resolve(input)).split(sep).join('/')
      : input
    const tests = findTestsGuardingSource(guards, sourcePath)
    if (!tests.length) {
      console.error(`No source guard tests read ${sourcePath}`)
      return 1
    }
    for (const testFile of tests) console.log(testFile)
    return 0
  }

  if (args.length) {
    console.error(`Unknown arguments: ${args.join(' ')} (see --help)`)
    return 2
  }

  const allSources = new Set()
  for (const [testFile, sources] of guards) {
    console.log(`${testFile}:`)
    for (const source of sources) {
      console.log(`  ${source}`)
      allSources.add(source)
    }
  }
  console.log(`Summary: ${guards.size} test files guard ${allSources.size} source files.`)
  return 0
}

// 通过 exitCode 而不是 process.exit 退出，确保 stdout 完整刷新后再结束进程。
const isMain = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)
if (isMain) process.exitCode = await runCli(process.argv)
