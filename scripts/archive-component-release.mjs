import { execFile } from 'node:child_process'
import { existsSync } from 'node:fs'
import { promisify } from 'node:util'
import { PublicKey, Signature } from '@threema/wasm-minisign-verify'
import { decodeTauriSignerPayload } from './verify-tauri-signature.mjs'
import {
  chmod,
  copyFile,
  cp,
  mkdir,
  readdir,
  readFile,
  realpath,
  rm,
  stat,
  writeFile,
} from 'node:fs/promises'
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { c as createTar, t as listTar } from 'tar'
import { assertReleaseComponent, readComponentVersion } from './release-components.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const component = assertReleaseComponent(process.argv[2])
const version = await readComponentVersion(root, component)
const platform =
  process.platform === 'win32' ? 'windows' : process.platform === 'darwin' ? 'darwin' : 'linux'
const arch = process.arch === 'x64' ? 'x86_64' : process.arch === 'arm64' ? 'aarch64' : process.arch
const executableSuffix = process.platform === 'win32' ? '.exe' : ''
const outputDir = join(root, 'release', 'component-artifacts')
const stageRoot = join(root, 'release', 'component-stage')
const directoryName = `pisper-${component}-${version}-${platform}-${arch}`
const stage = join(stageRoot, directoryName)
const rustRuntime =
  existsSync(join(root, 'runtime-rs/Cargo.toml')) && !existsSync(join(root, 'runtime/index.mjs'))
const requireSignature = process.argv.includes('--require-signature')
const label =
  component === 'desktop'
    ? 'Desktop'
    : component === 'tui'
      ? 'TUI'
      : rustRuntime
        ? 'Runtime_Rust'
        : 'Runtime'
const archive = join(outputDir, `Pisper_${label}_${version}_${platform}_${arch}.tar.gz`)

async function copyComponentTree(source, destination) {
  const sourceRoot = await realpath(source)
  const launchers = []
  async function inspect(directory, ancestors = new Set()) {
    const canonical = await realpath(directory)
    if (ancestors.has(canonical)) throw new Error(`Component contains a link cycle: ${directory}`)
    const nextAncestors = new Set([...ancestors, canonical])
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      let info = entry
      if (entry.isSymbolicLink()) {
        const target = await realpath(path)
        const targetPath = relative(sourceRoot, target)
        if (isAbsolute(targetPath) || targetPath === '..' || targetPath.startsWith(`..${sep}`)) {
          throw new Error(`Component link escapes its source root: ${path}`)
        }
        info = await stat(path)
        if (info.isFile() && basename(directory) === '.bin') {
          launchers.push({ path: relative(source, path), target: targetPath })
        }
      }
      if (info.isDirectory()) await inspect(path, nextAncestors)
      else if (!info.isFile()) throw new Error(`Unsupported component source entry: ${path}`)
    }
  }
  await inspect(source)
  await cp(source, destination, { recursive: true, force: true, dereference: true })
  for (const launcher of launchers) {
    const path = join(destination, launcher.path)
    const target = relative(dirname(path), join(destination, launcher.target)).replaceAll('\\', '/')
    // 不能把 npm bin 的脚本直接复制到 .bin，否则脚本内相对 import/require 会从错误目录解析。
    const quoted = `'${target.replaceAll("'", "'\\''")}'`
    await writeFile(path, `#!/bin/sh\nexec "$(dirname "$0")"/${quoted} "$@"\n`)
    await chmod(path, 0o755)
  }
}

async function stageDesktop() {
  await copyComponentTree(join(root, 'dist'), join(stage, 'dist'))
  await writeFile(
    join(stage, 'manifest.json'),
    `${JSON.stringify(
      {
        name: 'pisper-desktop',
        version,
        platform,
        arch,
        command: 'dist/index.html',
        layout: ['dist/'],
      },
      null,
      2,
    )}\n`,
    'utf8',
  )
}

async function stageRuntime() {
  if (rustRuntime) {
    await copyComponentTree(join(root, 'release/rust-runtime-component'), stage)
    const manifest = JSON.parse(await readFile(join(stage, 'manifest.json'), 'utf8'))
    if (
      manifest.backend !== 'rust' ||
      manifest.version !== version ||
      manifest.platform !== platform ||
      manifest.arch !== arch
    ) {
      throw new Error('Rust Runtime staging does not match the release version and platform.')
    }
    return
  }
  const seaRoot = join(root, 'release', 'sea')
  const sidecar = join(seaRoot, `pisper-sidecar${executableSuffix}`)
  const runtime = join(seaRoot, 'runtime')
  await Promise.all([stat(sidecar), stat(runtime)])
  await Promise.all([
    copyFile(sidecar, join(stage, `pisper-sidecar${executableSuffix}`)),
    copyComponentTree(runtime, join(stage, 'sidecar-runtime')),
    copyFile(
      join(seaRoot, 'runtime-size-manifest.json'),
      join(stage, 'runtime-size-manifest.json'),
    ),
  ])
  if (process.platform !== 'win32') {
    await chmod(join(stage, 'pisper-sidecar'), 0o755)
  }
  await writeFile(
    join(stage, 'manifest.json'),
    `${JSON.stringify(
      {
        name: 'pisper-runtime',
        version,
        platform,
        arch,
        command: `pisper-sidecar${executableSuffix}`,
        layout: ['pisper-sidecar', 'sidecar-runtime/'],
      },
      null,
      2,
    )}\n`,
    'utf8',
  )
}

async function stageTui() {
  const source = resolve(
    root,
    process.env.PISPER_TUI_STAGE_DIR ||
      join(
        'release',
        'tui',
        `pisper-${version}-${rustRuntime ? platform : platform === 'darwin' ? 'macos' : platform}-${arch}`,
      ),
  )
  await stat(source)
  await copyComponentTree(source, stage)
}

async function createArchive(sourceDirectory, destination) {
  await rm(destination, { force: true })
  // portable 避免宿主 tar 写入 macOS 扩展属性等与组件安装无关的元数据。
  await createTar({ cwd: stageRoot, file: destination, gzip: true, portable: true }, [
    sourceDirectory,
  ])
  let unsupportedEntry
  await listTar({
    file: destination,
    strict: true,
    onReadEntry(entry) {
      if (entry.type !== 'File' && entry.type !== 'Directory') {
        unsupportedEntry ??= `${entry.path} (${entry.type})`
      }
    },
  })
  if (unsupportedEntry) {
    await rm(destination, { force: true })
    throw new Error(`Component archive contains an unsupported entry: ${unsupportedEntry}`)
  }
  const bytes = (await stat(destination)).size
  if (bytes === 0) throw new Error(`Component archive is empty: ${destination}`)
  await signArchive(destination)
  console.log(`Packaged Pisper ${label}: ${destination} (${bytes} bytes)`)
}

async function signArchive(artifactPath) {
  const key = process.env.TAURI_SIGNING_PRIVATE_KEY?.trim()
  if (!key) {
    if (requireSignature)
      throw new Error('TAURI_SIGNING_PRIVATE_KEY is required for component releases.')
    return
  }
  // 只从环境传递私钥，日志和进程参数中均不包含密钥材料。
  await promisify(execFile)(
    process.execPath,
    [join(root, 'node_modules/@tauri-apps/cli/tauri.js'), 'signer', 'sign', artifactPath],
    { cwd: root, env: process.env },
  )
  const publicKey = PublicKey.decode(
    decodeTauriSignerPayload(
      await readFile(join(root, 'src-tauri/updater.pubkey'), 'utf8'),
    ).toString('utf8'),
  )
  const signature = Signature.decode(
    decodeTauriSignerPayload(await readFile(`${artifactPath}.sig`, 'utf8')).toString('utf8'),
  )
  try {
    if (!publicKey.verify(await readFile(artifactPath), signature))
      throw new Error('Component signature does not match the shipped updater public key.')
  } finally {
    signature.free()
    publicKey.free()
  }
}

await rm(stage, { recursive: true, force: true })
await mkdir(stage, { recursive: true })
await mkdir(outputDir, { recursive: true })
if (component === 'desktop') await stageDesktop()
else if (component === 'runtime') await stageRuntime()
else await stageTui()
await createArchive(directoryName, archive)

if (component === 'runtime' && !rustRuntime) {
  const nodeDirectoryName = `pisper-runtime-node-${version}-${platform}-${arch}`
  const nodeStage = join(stageRoot, nodeDirectoryName)
  const nodeArchive = join(outputDir, `Pisper_Runtime_Node_${version}_${platform}_${arch}.tar.gz`)
  await rm(nodeStage, { recursive: true, force: true })
  await mkdir(nodeStage, { recursive: true })
  await copyComponentTree(join(stage, 'sidecar-runtime'), join(nodeStage, 'sidecar-runtime'))
  await writeFile(
    join(nodeStage, 'manifest.json'),
    `${JSON.stringify(
      {
        name: 'pisper-runtime-node',
        version,
        platform,
        arch,
        command: 'sidecar-runtime/runtime/sidecar.mjs',
        layout: ['sidecar-runtime/'],
      },
      null,
      2,
    )}\n`,
    'utf8',
  )
  await createArchive(nodeDirectoryName, nodeArchive)
}

if (component === 'tui') {
  const thinDirectoryName = `pisper-tui-component-${version}-${platform}-${arch}`
  const thinStage = join(stageRoot, thinDirectoryName)
  const thinArchive = join(outputDir, `Pisper_TUI_Component_${version}_${platform}_${arch}.tar.gz`)
  await rm(thinStage, { recursive: true, force: true })
  await mkdir(thinStage, { recursive: true })
  await copyFile(
    join(stage, `pisper${executableSuffix}`),
    join(thinStage, `pisper${executableSuffix}`),
  )
  if (process.platform !== 'win32') await chmod(join(thinStage, 'pisper'), 0o755)
  await writeFile(
    join(thinStage, 'manifest.json'),
    `${JSON.stringify(
      {
        name: 'pisper-tui-component',
        backend: rustRuntime ? 'rust' : 'node',
        version,
        platform,
        arch,
        command: `pisper${executableSuffix}`,
        layout: ['pisper'],
      },
      null,
      2,
    )}\n`,
    'utf8',
  )
  await createArchive(thinDirectoryName, thinArchive)
}
