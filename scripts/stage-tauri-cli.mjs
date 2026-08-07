import { spawn } from 'node:child_process'
import { copyFile, mkdir, readFile, stat } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const executableSuffix = process.platform === 'win32' ? '.exe' : ''
const source = join(root, 'src-tui', 'target', 'release', `pisper${executableSuffix}`)

function targetTriples() {
  const arch =
    process.arch === 'x64' ? 'x86_64' : process.arch === 'arm64' ? 'aarch64' : process.arch
  if (process.platform === 'win32') {
    return [`${arch}-pc-windows-msvc`, `${arch}-pc-windows-gnu`]
  }
  if (process.platform === 'darwin') return [`${arch}-apple-darwin`]
  return [`${arch}-unknown-linux-gnu`]
}

function run(command, args) {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, args, { cwd: root, env: process.env, stdio: 'inherit' })
    child.once('error', rejectRun)
    child.once('exit', (code, signal) => {
      if (code === 0) resolveRun()
      else rejectRun(new Error(`${command} exited with ${signal || code}.`))
    })
  })
}

const tuiManifest = await readFile(join(root, 'src-tui', 'Cargo.toml'), 'utf8')
const tuiVersion = tuiManifest.match(/\[package\][\s\S]*?\r?\nversion\s*=\s*"([^"]+)"/)?.[1]
if (!tuiVersion) throw new Error('Unable to resolve the Pisper TUI version.')
await run('cargo', [
  'build',
  '--locked',
  '--release',
  '--manifest-path',
  join(root, 'src-tui', 'Cargo.toml'),
])
await stat(source).catch((error) => {
  if (error?.code === 'ENOENT') throw new Error('Pisper TUI release binary was not produced.')
  throw error
})

const binariesDir = join(root, 'src-tauri', 'binaries')
await mkdir(binariesDir, { recursive: true })
for (const triple of targetTriples()) {
  const destination = join(binariesDir, `pisper-cli-${triple}${executableSuffix}`)
  await copyFile(source, destination)
  console.log(`Prepared Tauri CLI payload: ${destination}`)
}
console.log(`Staged Pisper CLI ${tuiVersion} for the Tauri desktop bundle.`)
