import { spawn } from 'node:child_process'
import { copyFile, mkdir, stat } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const sandboxRoot = resolve(process.env.PISPER_SANDBOX_SOURCE_DIR || join(root, '..', 'sandbox'))
const executableSuffix = process.platform === 'win32' ? '.exe' : ''
const source = join(sandboxRoot, 'target', 'release', `agent-sandboxd${executableSuffix}`)

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
    const child = spawn(command, args, {
      cwd: sandboxRoot,
      env: process.env,
      stdio: 'inherit',
    })
    child.once('error', rejectRun)
    child.once('exit', (code, signal) => {
      if (code === 0) resolveRun()
      else rejectRun(new Error(`${command} exited with ${signal || code}.`))
    })
  })
}

await stat(join(sandboxRoot, 'Cargo.toml')).catch((error) => {
  if (error?.code === 'ENOENT') {
    throw new Error(
      `Agent Sandbox Runtime source was not found at ${sandboxRoot}. Set PISPER_SANDBOX_SOURCE_DIR.`,
    )
  }
  throw error
})
await run('cargo', ['build', '--locked', '--release', '--workspace'])
await stat(source).catch((error) => {
  if (error?.code === 'ENOENT') throw new Error('agent-sandboxd release binary was not produced.')
  throw error
})

const binariesDir = join(root, 'src-tauri', 'binaries')
await mkdir(binariesDir, { recursive: true })
for (const triple of targetTriples()) {
  const destination = join(binariesDir, `agent-sandboxd-${triple}${executableSuffix}`)
  await copyFile(source, destination)
  console.log(`Prepared Tauri Agent Sandbox payload: ${destination}`)
}
