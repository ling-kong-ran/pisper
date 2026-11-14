// 移动 Runtime 按文件裁剪 node_modules：从 bundle 产物出发追踪真正会被加载的文件，
// 其余的（另一种模块格式的副本、浏览器构建、编码器、未用到的子模块等）一律删掉。
// 包体积按包整删只能去掉整包不用的依赖，留下的包里往往一半以上是用不到的文件。
//
// 追踪看不见用运行时拼出来的路径，所以两类文件直接列为追踪起点：
// - Pi 引擎（@earendil-works/*）自己的 dist：Pisper 按计算出的 URL 加载它的子模块，
//   Pi 也用拼接路径懒加载 Provider 和 OAuth 模块；
// - criticalRuntimeEntries 里的关键文件（如 pdfjs 的 worker）。
// 每个包的 package.json 和许可证文件始终保留。
//
// 裁剪前后各在独立 Node 进程里把这些起点逐个 import 一遍：裁剪前能加载、裁剪后加载失败的
// 模块说明追踪漏了文件，直接让构建失败，不把缺文件的 Runtime 装进安装包。
import { execFile } from 'node:child_process'
import { nodeFileTrace } from '@vercel/nft'
import { mkdtemp, readFile, readdir, rm, rmdir, stat, writeFile } from 'node:fs/promises'
import { isBuiltin } from 'node:module'
import { tmpdir } from 'node:os'
import { join, relative, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { promisify } from 'node:util'

const run = promisify(execFile)

const PI_SCOPE = 'node_modules/@earendil-works'
// Pi CLI 的独立打包副本只供 RPC 入口使用，Pisper 从不加载。
const PI_SKIPPED_DIRECTORIES = ['pi-coding-agent/dist/bundle/']
const MODULE_SUFFIXES = ['.js', '.mjs', '.cjs']
const LICENSE_FILE = /^(?:license|licence|copying|notice)(?:[._-].*)?$/i

function posixPath(path) {
  return path.split(sep).join('/')
}

async function listFiles(directory) {
  const files = []
  let entries
  try {
    entries = await readdir(directory, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return files
    throw error
  }
  for (const entry of entries) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) files.push(...(await listFiles(path)))
    else if (entry.isFile()) files.push(path)
  }
  return files
}

function isPiSkipped(relativePath) {
  const scoped = relativePath.slice(`${PI_SCOPE}/`.length)
  return PI_SKIPPED_DIRECTORIES.some((prefix) => scoped.startsWith(prefix))
}

function isPiOwnFile(relativePath) {
  if (!relativePath.startsWith(`${PI_SCOPE}/`)) return false
  // Pi 包里嵌套的 node_modules 是第三方依赖，照常按追踪结果裁剪。
  return !relativePath.slice(`${PI_SCOPE}/`.length).includes('/node_modules/')
}

async function removeEmptyDirectories(directory) {
  let entries
  try {
    entries = await readdir(directory, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return true
    throw error
  }
  let empty = true
  for (const entry of entries) {
    if (!entry.isDirectory() || !(await removeEmptyDirectories(join(directory, entry.name))))
      empty = false
  }
  if (empty) await rmdir(directory)
  return empty
}

// 子进程把能加载的模块写进结果文件；模块的副作用（定时器、句柄）随进程退出一起结束。
// 个别模块加载时会打印或调用 process.exit，所以不用 stdout 传结果，也先拦下 exit。
const LOAD_PROBE = `
import { readFileSync, writeFileSync } from 'node:fs'
const exit = process.exit.bind(process)
process.exit = () => { throw new Error('process.exit during import') }
const loaded = []
for (const specifier of JSON.parse(readFileSync(process.env.PISPER_LOAD_PROBE, 'utf8'))) {
  try {
    await import(specifier)
    loaded.push([specifier, import.meta.resolve(specifier)])
  } catch {}
}
writeFileSync(process.env.PISPER_LOAD_PROBE_RESULT, JSON.stringify(loaded))
exit(0)
`

// bundle 产物里对外部包的 import（含懒加载）就是 Runtime 实际用到的外部入口。
const EXTERNAL_IMPORT = /(?:\bfrom\s*|\bimport\s*\(\s*)["']([^"'./][^"']*)["']/g

async function externalSpecifiers(files) {
  const specifiers = new Set()
  for (const path of files) {
    for (const [, specifier] of (await readFile(path, 'utf8')).matchAll(EXTERNAL_IMPORT)) {
      if (!isBuiltin(specifier)) specifiers.add(specifier)
    }
  }
  return [...specifiers].sort()
}

async function loadableModules(runtimeDir, specifiers) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-load-probe-'))
  const input = join(directory, 'specifiers.json')
  const result = join(directory, 'loaded.json')
  try {
    // 几百个绝对路径会超过单个环境变量的长度上限，列表经文件传递。
    await writeFile(input, JSON.stringify(specifiers))
    await run(process.execPath, ['--input-type=module', '-e', LOAD_PROBE], {
      cwd: runtimeDir,
      env: {
        ...process.env,
        PISPER_LOAD_PROBE: input,
        PISPER_LOAD_PROBE_RESULT: result,
      },
      maxBuffer: 32 * 1024 * 1024,
      timeout: 300_000,
    })
    return new Map(JSON.parse(await readFile(result, 'utf8')))
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
}

// 命令行入口加载即执行（打印用法、读参数），不能当普通模块探测。
async function isCommandEntry(path) {
  return (await readFile(path, 'utf8')).startsWith('#!')
}

export async function traceRuntimeFiles({ runtimeDir, criticalPaths = [] }) {
  const nodeModules = join(runtimeDir, 'node_modules')
  const runtimeFiles = (await listFiles(join(runtimeDir, 'runtime'))).filter((path) =>
    MODULE_SUFFIXES.some((suffix) => path.endsWith(suffix)),
  )
  const piFiles = (await listFiles(join(runtimeDir, PI_SCOPE))).filter((path) => {
    const relativePath = posixPath(relative(runtimeDir, path))
    return isPiOwnFile(relativePath) && !isPiSkipped(relativePath)
  })
  const roots = [
    ...runtimeFiles,
    ...piFiles.filter((path) => MODULE_SUFFIXES.some((suffix) => path.endsWith(suffix))),
    ...criticalPaths
      .filter((path) => path.startsWith('node_modules/'))
      .map((path) => join(runtimeDir, path)),
  ]
  const probes = await externalSpecifiers(runtimeFiles)
  for (const path of roots) {
    if (!runtimeFiles.includes(path) && !(await isCommandEntry(path)))
      probes.push(pathToFileURL(path).href)
  }
  const loadedBefore = await loadableModules(runtimeDir, probes)
  // 外部包入口按 Node 实际解析到的文件加入追踪，bundle 里的懒加载 import 追踪器未必看得到。
  for (const [specifier, resolved] of loadedBefore) {
    if (!specifier.startsWith('file:') && resolved.startsWith('file:'))
      roots.push(fileURLToPath(resolved))
  }
  const { fileList } = await nodeFileTrace(roots, {
    base: runtimeDir,
    processCwd: runtimeDir,
    mixedModules: true,
  })
  const keep = new Set(
    [...fileList].map(posixPath).filter((path) => path.startsWith('node_modules/')),
  )
  for (const path of piFiles) keep.add(posixPath(relative(runtimeDir, path)))
  for (const path of criticalPaths) keep.add(path)

  const removed = { bytes: 0, fileCount: 0 }
  for (const path of await listFiles(nodeModules)) {
    const relativePath = posixPath(relative(runtimeDir, path))
    const name = relativePath.slice(relativePath.lastIndexOf('/') + 1)
    if (keep.has(relativePath) || name === 'package.json' || LICENSE_FILE.test(name)) continue
    removed.bytes += (await stat(path)).size
    removed.fileCount += 1
    await rm(path, { force: true })
  }
  await removeEmptyDirectories(nodeModules)

  const loadedAfter = await loadableModules(runtimeDir, probes)
  const broken = [...loadedBefore.keys()].filter((specifier) => !loadedAfter.has(specifier))
  if (broken.length) {
    const names = broken
      .slice(0, 10)
      .map((specifier) =>
        specifier.startsWith('file:')
          ? posixPath(relative(runtimeDir, fileURLToPath(specifier)))
          : specifier,
      )
    throw new Error(
      `Mobile Runtime file trimming broke ${broken.length} module(s): ${names.join(', ')}`,
    )
  }
  return { ...removed, probedModules: probes.length, loadedModules: loadedAfter.size }
}
