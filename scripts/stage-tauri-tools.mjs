// 打包前把 Tauri 打包要用的工具放进它的工具目录。Tauri 打包时自己从 github.com 的 releases/download
// 下载这些工具，那个入口整段时间回 504 时整个桌面发版失败（2026-11-08 Windows 卡在 NSIS）。
// 这里用 github-download.mjs 下载同一批文件（入口不通时走 GitHub API），Tauri 看到工具已在就不再下载：
// - Windows：NSIS 3.11 与 nsis_tauri_utils 插件，按 Tauri 自己的 SHA1 校验后放进 %LOCALAPPDATA%\tauri\NSIS；
// - Linux：AppImage 用的 AppRun、linuxdeploy 与插件放进 ~/.cache/tauri（Tauri 只看文件在不在，也不校验）。
// 地址与摘要抄自 tauri-bundler 2.9.x（src/bundle/windows/nsis/mod.rs、linux/appimage/linuxdeploy.rs），
// 对应锁定的 @tauri-apps/cli 2.11.4；升级 CLI 时守卫测试会要求同步这里。
import { execFile } from 'node:child_process'
import { createHash } from 'node:crypto'
import { chmod, mkdir, mkdtemp, readFile, rename, rm, stat, writeFile } from 'node:fs/promises'
import { join, posix, win32 } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'
import { downloadReleaseAsset } from './github-download.mjs'

const run = promisify(execFile)

export const TAURI_CLI_VERSION = '2.11.4'
export const NSIS = {
  url: 'https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip',
  sha1: 'ef7ff767e5cbd9edd22add3a32c9b8f4500bb10d',
  root: 'nsis-3.11',
}
export const NSIS_TAURI_UTILS = {
  url: 'https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll',
  sha1: '75197fee3c6a814fe035788d1c34ead39349b860',
  path: 'Plugins/x86-unicode/additional/nsis_tauri_utils.dll',
}
// Tauri 判断工具链齐全时检查的文件（同一份源码里的 NSIS_REQUIRED_FILES）。
export const NSIS_REQUIRED_FILES = [
  'makensis.exe',
  'Bin/makensis.exe',
  'Stubs/lzma-x86-unicode',
  'Stubs/lzma_solid-x86-unicode',
  NSIS_TAURI_UTILS.path,
  'Include/MUI2.nsh',
  'Include/FileFunc.nsh',
  'Include/x64.nsh',
  'Include/nsDialogs.nsh',
  'Include/WinMessages.nsh',
  'Include/Win/COM.nsh',
  'Include/Win/Propkey.nsh',
  'Include/Win/RestartManager.nsh',
]

function sha1(bytes) {
  return createHash('sha1').update(bytes).digest('hex')
}

function sha1Check(name, expected) {
  return (bytes) => {
    const actual = sha1(bytes)
    if (actual !== expected) throw new Error(`${name} SHA1 mismatch: ${actual}`)
  }
}

async function exists(path) {
  return Boolean(await stat(path).catch(() => null))
}

async function toolsetReady(toolset, pluginSha1 = NSIS_TAURI_UTILS.sha1) {
  for (const file of NSIS_REQUIRED_FILES) if (!(await exists(join(toolset, file)))) return false
  const plugin = await readFile(join(toolset, NSIS_TAURI_UTILS.path)).catch(() => null)
  return Boolean(plugin && sha1(plugin) === pluginSha1)
}

// Tauri 的工具目录：dirs::cache_dir() 下的 tauri（Windows 是 %LOCALAPPDATA%，Linux 是 XDG 缓存目录）。
export function tauriToolsDirectory(env = process.env, platform = process.platform) {
  if (platform === 'win32') {
    if (!env.LOCALAPPDATA) throw new Error('LOCALAPPDATA is not set')
    return win32.join(env.LOCALAPPDATA, 'tauri')
  }
  const cache = env.XDG_CACHE_HOME || (env.HOME ? posix.join(env.HOME, '.cache') : '')
  if (!cache) throw new Error('Neither XDG_CACHE_HOME nor HOME is set')
  return posix.join(cache, 'tauri')
}

// AppImage 打包用的工具（Linux x86_64）。Tauri 只检查文件是否存在；最后一个是可选的，下不到时 Tauri 用内置版本。
export function appImageTools(arch = 'x86_64') {
  return [
    {
      file: `AppRun-${arch}`,
      url: `https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-${arch}`,
    },
    {
      file: `linuxdeploy-${arch}.AppImage`,
      url: `https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy/linuxdeploy-${arch}.AppImage`,
    },
    {
      file: 'linuxdeploy-plugin-gtk.sh',
      url: 'https://raw.githubusercontent.com/tauri-apps/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh',
    },
    {
      file: 'linuxdeploy-plugin-gstreamer.sh',
      url: 'https://raw.githubusercontent.com/tauri-apps/linuxdeploy-plugin-gstreamer/master/linuxdeploy-plugin-gstreamer.sh',
    },
    {
      file: 'linuxdeploy-plugin-appimage.AppImage',
      url: `https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-${arch}.AppImage`,
      optional: true,
    },
  ]
}

export async function stageTauriAppImageTools({
  toolsDir = tauriToolsDirectory(),
  arch = 'x86_64',
  fetchImpl = globalThis.fetch,
  retryDelayMs,
} = {}) {
  await mkdir(toolsDir, { recursive: true })
  const staged = []
  for (const tool of appImageTools(arch)) {
    const target = join(toolsDir, tool.file)
    if (await exists(target)) continue
    let bytes
    try {
      bytes = await downloadReleaseAsset(tool.url, {
        fetchImpl,
        ...(retryDelayMs === undefined ? {} : { retryDelayMs }),
        timeoutMs: 600_000,
        maxBytes: 256 * 1024 * 1024,
        verify: (value) => {
          if (!value.length) throw new Error(`empty download: ${tool.url}`)
        },
      })
    } catch (error) {
      if (tool.optional) continue
      throw error
    }
    // 先写临时名再改名：中途失败不会留下一个“存在但残缺”的文件骗过 Tauri。
    await writeFile(`${target}.partial`, bytes)
    await chmod(`${target}.partial`, 0o755)
    await rename(`${target}.partial`, target)
    staged.push(tool.file)
  }
  return { staged, toolsDir }
}

export async function stageTauriNsis({
  toolsDir = tauriToolsDirectory(),
  nsis = NSIS,
  plugin: pluginAsset = NSIS_TAURI_UTILS,
  fetchImpl = globalThis.fetch,
  retryDelayMs,
  // Windows 的 tar（bsdtar）能解 ZIP；测试里可以换掉。
  extractZip = (archive, cwd) => run('tar', ['-xf', archive], { cwd, timeout: 120_000 }),
} = {}) {
  const toolset = join(toolsDir, 'NSIS')
  if (await toolsetReady(toolset, pluginAsset.sha1)) return { staged: false, toolset }
  const download = (asset, name) =>
    downloadReleaseAsset(asset.url, {
      fetchImpl,
      ...(retryDelayMs === undefined ? {} : { retryDelayMs }),
      timeoutMs: 600_000,
      maxBytes: 64 * 1024 * 1024,
      verify: sha1Check(name, asset.sha1),
    })
  const [archive, plugin] = await Promise.all([
    download(nsis, `${nsis.root}.zip`),
    download(pluginAsset, 'nsis_tauri_utils.dll'),
  ])
  await mkdir(toolsDir, { recursive: true })
  const temporary = await mkdtemp(join(toolsDir, 'nsis-staging-'))
  try {
    const zip = join(temporary, 'nsis.zip')
    await writeFile(zip, archive)
    await extractZip(zip, temporary)
    // 残缺的旧目录（Tauri 自己也会这么处理）整体换掉。
    await rm(toolset, { recursive: true, force: true })
    await rename(join(temporary, nsis.root), toolset)
  } finally {
    await rm(temporary, { recursive: true, force: true })
  }
  const pluginPath = join(toolset, NSIS_TAURI_UTILS.path)
  await mkdir(join(pluginPath, '..'), { recursive: true })
  await writeFile(pluginPath, plugin)
  if (!(await toolsetReady(toolset, pluginAsset.sha1)))
    throw new Error(`Staged NSIS toolset is incomplete: ${toolset}`)
  return { staged: true, toolset }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  if (process.platform === 'win32') {
    const { staged, toolset } = await stageTauriNsis()
    console.log(staged ? `Staged NSIS toolset at ${toolset}` : `NSIS toolset ready at ${toolset}`)
  } else if (process.platform === 'linux' && process.arch === 'x64') {
    const { staged, toolsDir } = await stageTauriAppImageTools()
    console.log(
      `AppImage tools ready at ${toolsDir}${staged.length ? ` (staged ${staged.join(', ')})` : ''}`,
    )
  }
}
