import { spawn } from 'node:child_process'
import { readFile, rm } from 'node:fs/promises'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const tauriCli = path.join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js')
const updaterConfig = path.join(root, 'src-tauri', 'tauri.updater.conf.json')
const localKeyPath = path.join(root, 'release', 'tauri-updater.key')
const localPasswordPath = path.join(root, 'release', 'tauri-updater-key.password')
let bundleDir = path.join(root, 'src-tauri', 'target', 'release', 'bundle')
const stageDir = path.resolve(
  root,
  process.env.PISPER_TAURI_STAGE_DIR || path.join('release', 'tauri-artifacts'),
)
const bundlesByPlatform = {
  darwin: 'app,dmg',
  linux: 'appimage,deb',
  win32: 'nsis',
}
const bundles = bundlesByPlatform[process.platform]
if (!bundles) throw new Error(`Unsupported Tauri release platform: ${process.platform}`)

function run(command, args, env = process.env) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: root, env, stdio: 'inherit' })
    child.once('error', reject)
    child.once('exit', (code, signal) => {
      if (code === 0) resolve()
      else reject(new Error(`${path.basename(command)} exited with ${signal || code}.`))
    })
  })
}

function capture(command, args, env = process.env) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8').on('data', (chunk) => (stdout += chunk))
    child.stderr.setEncoding('utf8').on('data', (chunk) => (stderr += chunk))
    child.once('error', reject)
    child.once('exit', (code, signal) => {
      if (code === 0) resolve(stdout)
      else reject(new Error(`${path.basename(command)} exited with ${signal || code}: ${stderr}`))
    })
  })
}

async function optionalFile(filePath) {
  try {
    return await readFile(filePath, 'utf8')
  } catch (error) {
    if (error?.code === 'ENOENT') return ''
    throw error
  }
}

// Tauri embeds dist as-is. Rebuild and audit it here so a prior dev server
// cannot leak development JSX into an otherwise successful installer.
await run(process.execPath, [path.join(root, 'scripts', 'build-frontend.mjs')])
await run(process.execPath, [path.join(root, 'scripts', 'check-bundle-budget.mjs')])

const desktopPackage = JSON.parse(
  await readFile(path.join(root, 'src-tauri', 'desktop-package.json'), 'utf8'),
)
const tauriConfig = JSON.parse(
  await readFile(path.join(root, 'src-tauri', 'tauri.conf.json'), 'utf8'),
)
if (tauriConfig.version !== 'desktop-package.json') {
  throw new Error(
    'src-tauri/tauri.conf.json must read its release version from desktop-package.json.',
  )
}

const env = { ...process.env }
for (const name of [
  'APPLE_API_ISSUER',
  'APPLE_API_KEY',
  'APPLE_API_KEY_PATH',
  'APPLE_CERTIFICATE',
  'APPLE_CERTIFICATE_PASSWORD',
  'APPLE_SIGNING_IDENTITY',
]) {
  if (!String(env[name] || '').trim()) delete env[name]
}

let signingKey = env.TAURI_SIGNING_PRIVATE_KEY || ''
let signingPassword = env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD || ''
let credentialSource = 'environment'

if (!signingKey.trim()) {
  signingKey = await optionalFile(localKeyPath)
  signingPassword = await optionalFile(localPasswordPath)
  credentialSource = 'ignored release files'
}

const tauriTargetArgs = []
if (process.platform === 'win32') {
  const rustVersion = await capture(process.env.RUSTC || 'rustc', ['-vV'], env)
  const rustTarget = rustVersion.match(/^host:\s*(\S+)$/m)?.[1] || ''
  if (!rustTarget) throw new Error('Unable to resolve the Rust host target from rustc -vV.')
  if (rustTarget.endsWith('-windows-gnu')) {
    tauriTargetArgs.push('--target', rustTarget)
    bundleDir = path.join(root, 'src-tauri', 'target', rustTarget, 'release', 'bundle')
  }
}
env.PISPER_TAURI_BUNDLE_DIR = bundleDir
await rm(bundleDir, { recursive: true, force: true })

const buildArgs = [tauriCli, 'build', '--bundles', bundles, ...tauriTargetArgs]
if (signingKey.trim()) {
  env.TAURI_SIGNING_PRIVATE_KEY = signingKey
  env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = signingPassword.trim()
  console.log(`Building signed Tauri updater artifacts using ${credentialSource}.`)
  buildArgs.push('--config', updaterConfig)
} else {
  delete env.TAURI_SIGNING_PRIVATE_KEY
  delete env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD
  console.log(`Updater signing credentials are unavailable; building unsigned ${bundles} bundles.`)
}

await run(process.execPath, buildArgs, env)
const stageArgs = [path.join(root, 'scripts', 'stage-tauri-artifacts.mjs')]
if (signingKey.trim()) stageArgs.push('--require-signature')
await run(process.execPath, stageArgs, env)
if (signingKey.trim()) {
  await run(
    process.execPath,
    [
      path.join(root, 'scripts', 'create-tauri-update-manifest.mjs'),
      `v${desktopPackage.version}`,
      stageDir,
    ],
    env,
  )
}
