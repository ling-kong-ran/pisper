import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { copyFile, mkdir, readFile, realpath, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { existsSync } from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { stageRustSpeech } from './stage-rust-speech.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const buildDir = path.join(root, 'release', 'rust-build')
const runtimeDir = path.join(root, 'release', 'rust-runtime')
const binariesDir = path.join(root, 'src-tauri', 'binaries')
const runtimeManifest = path.join(root, 'runtime-rs', 'Cargo.toml')
const tuiManifest = path.join(root, 'src-tui', 'Cargo.toml')
const desktopManifest = path.join(root, 'src-tauri', 'Cargo.toml')
const executableSuffix = process.platform === 'win32' ? '.exe' : ''
const runtimeExecutable = path.join(buildDir, `pisper-server${executableSuffix}`)
const overlayPath = path.join(buildDir, 'tauri-rust.conf.json')
const updaterConfig = path.join(root, 'src-tauri', 'tauri.updater.conf.json')
const tauriCli = path.join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js')
const cargo = process.env.CARGO || 'cargo'

if (process.platform !== 'win32' && !(process.platform === 'darwin' && process.arch === 'arm64')) {
  throw new Error('Rust desktop packaging supports Windows x64 and macOS Apple Silicon.')
}

function run(command, args, { env = process.env, capture = false } = {}) {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, args, {
      cwd: root,
      env,
      stdio: capture ? ['ignore', 'pipe', 'pipe'] : 'inherit',
    })
    let stdout = ''
    let stderr = ''
    if (capture) {
      child.stdout.setEncoding('utf8').on('data', (chunk) => (stdout += chunk))
      child.stderr.setEncoding('utf8').on('data', (chunk) => (stderr += chunk))
    }
    child.once('error', rejectRun)
    child.once('close', (code, signal) => {
      if (code === 0) resolveRun(stdout.trim())
      else {
        rejectRun(
          new Error(
            `${path.basename(command)} exited with ${signal || code}.${stderr ? ` ${stderr.trim()}` : ''}${
              capture && stdout ? `\n--- captured stdout (tail) ---\n${stdout.slice(-2500)}` : ''
            }`,
          ),
        )
      }
    })
  })
}

const desktopPackage = JSON.parse(
  await readFile(path.join(root, 'src-tauri', 'desktop-package.json'), 'utf8'),
)
const runtimeVersion = desktopPackage.bundled?.runtime
if (typeof runtimeVersion !== 'string' || !runtimeVersion.trim()) {
  throw new Error('desktop-package.json must declare bundled.runtime before packaging.')
}
const rustVersion = await run(process.env.RUSTC || 'rustc', ['-vV'], { capture: true })
const rustTarget = rustVersion.match(/^host:\s*(\S+)$/m)?.[1]
if (
  !rustTarget ||
  !(process.platform === 'win32'
    ? /-windows-(msvc|gnu)$/.test(rustTarget)
    : rustTarget === 'aarch64-apple-darwin')
) {
  throw new Error('Rust host target does not match the desktop packaging platform.')
}
const sourceCommit = await run('git', ['rev-parse', 'HEAD'], { capture: true })
if (!/^[0-9a-f]{40,64}$/i.test(sourceCommit)) {
  throw new Error('Unable to resolve the source commit from git rev-parse HEAD.')
}

// 每个组件使用自己的 target，避免外部 CARGO_TARGET_DIR 改变待暂存产物的位置。
const cargoEnv = { ...process.env }
// macOS 的 /var 是系统链接；安全路径测试的临时根必须使用真实路径，保留拒绝链接的生产检查。
if (process.platform === 'darwin') cargoEnv.TMPDIR = await realpath(tmpdir())
delete cargoEnv.CARGO_BUILD_TARGET
const runtimeEnv = { ...cargoEnv, CARGO_TARGET_DIR: path.join(root, 'runtime-rs', 'target') }
const tuiEnv = { ...cargoEnv, CARGO_TARGET_DIR: path.join(root, 'src-tui', 'target') }
const tauriEnv = { ...cargoEnv, CARGO_TARGET_DIR: path.join(root, 'src-tauri', 'target') }
// 资源/PTY 测试与最终 App 使用同一部署目标，避免验证较低目标后再替换原生依赖。
if (process.platform === 'darwin') tauriEnv.MACOSX_DEPLOYMENT_TARGET = '14.0'
const signedUpdater = Boolean(tauriEnv.TAURI_SIGNING_PRIVATE_KEY?.trim())
// macOS 本地资源验收复用即将打包的 release 产物，避免维护第二套大型编译缓存。
const desktopTestProfileArgs = process.platform === 'darwin' ? ['--release'] : []

// 与现有桌面打包入口一致，先重建生产前端，再执行体积和 WebView 语法审计。
for (const script of ['build-frontend.mjs', 'check-bundle-budget.mjs', 'check-dist-compat.mjs']) {
  await run(process.execPath, [path.join(root, 'scripts', script)])
}

await run(cargo, ['test', '--locked', '--manifest-path', runtimeManifest], { env: runtimeEnv })
await run(
  cargo,
  [
    'test',
    '--locked',
    '--manifest-path',
    runtimeManifest,
    '--package',
    'pi-rs',
    '--test',
    'pisper_mcp_regression',
    '--test',
    'pisper_model_registry_regression',
  ],
  { env: runtimeEnv },
)
await mkdir(buildDir, { recursive: true })
// Windows 不覆盖可能运行中的默认 exe；macOS 使用 Cargo 的标准命名，
// 避免 rustc -o 在多 emit 类型下附加 crate hash 后找不到暂存入口。
if (process.platform === 'darwin') {
  await run(cargo, ['build', '--release', '--locked', '--manifest-path', runtimeManifest], {
    env: runtimeEnv,
  })
  await copyFile(
    path.join(root, 'runtime-rs', 'target', 'release', 'pisper-server'),
    runtimeExecutable,
  )
} else {
  await run(
    cargo,
    [
      'rustc',
      '--release',
      '--locked',
      '--manifest-path',
      runtimeManifest,
      '--',
      '-o',
      runtimeExecutable,
    ],
    { env: runtimeEnv },
  )
}
// 准备原生资源，后续桌面资源检查与生产验收使用同一目录。
await mkdir(runtimeDir, { recursive: true })
const speechResources = await stageRustSpeech({ root, targetDir: runtimeDir })
const teleaSource = await readFile(
  path.join(root, 'runtime-rs/src/native_image_runtime/telea.rs'),
  'utf8',
)
const teleaLicense = teleaSource.match(/\/\*\r?\n(Intel License Agreement[\s\S]*?)\r?\n\*\//)?.[1]
if (!teleaLicense) throw new Error('Native Telea license must accompany binary distributions.')
await mkdir(path.join(runtimeDir, 'licenses'), { recursive: true })
await writeFile(path.join(runtimeDir, 'licenses/OPENCV-TELEA.txt'), `${teleaLicense}\n`)
await run(cargo, ['build', '--release', '--locked', '--manifest-path', tuiManifest], {
  env: tuiEnv,
})

await mkdir(binariesDir, { recursive: true })
await copyFile(
  runtimeExecutable,
  path.join(binariesDir, `pisper-sidecar-${rustTarget}${executableSuffix}`),
)
await copyFile(
  path.join(root, 'src-tui', 'target', 'release', `pisper${executableSuffix}`),
  path.join(binariesDir, `pisper-cli-${rustTarget}${executableSuffix}`),
)
await mkdir(runtimeDir, { recursive: true })
await writeFile(
  path.join(runtimeDir, 'package.json'),
  `${JSON.stringify(
    {
      name: 'pisper-runtime',
      version: runtimeVersion,
      backend: 'rust',
      sourceCommit,
      sourceDirty: Boolean(
        await run('git', ['status', '--porcelain', '--untracked-files=no'], { capture: true }),
      ),
      executableSha256: createHash('sha256')
        .update(await readFile(runtimeExecutable))
        .digest('hex'),
      speechResources,
      nativeImageLicense: {
        path: 'licenses/OPENCV-TELEA.txt',
        source: 'https://github.com/opencv/opencv/blob/5.0.0/modules/photo/src/inpaint.cpp',
        sha256: createHash('sha256').update(`${teleaLicense}\n`).digest('hex'),
      },
    },
    null,
    2,
  )}\n`,
)

await run(process.execPath, [
  path.join(root, 'scripts', 'smoke-rust-provider-upgrade.mjs'),
  runtimeExecutable,
])

// Tauri 按 RFC 7396 合并 overlay：null 删除旧 SEA 资源键，其余资源（含 dist）保留。
await writeFile(
  overlayPath,
  `${JSON.stringify(
    {
      bundle: {
        ...(process.platform === 'darwin'
          ? {
              macOS: {
                minimumSystemVersion: '14.0',
                signingIdentity: process.env.APPLE_SIGNING_IDENTITY?.trim() || '-',
              },
            }
          : {}),
        resources: {
          '../release/sea/runtime/': null,
          '../release/rust-runtime/': 'sidecar-runtime/',
        },
      },
    },
    null,
    2,
  )}\n`,
)

const targetArgs = rustTarget.endsWith('-windows-gnu') ? ['--target', rustTarget] : []
const bundleDir = path.join(
  root,
  'src-tauri',
  'target',
  ...(targetArgs.length ? [rustTarget] : []),
  'release',
  'bundle',
)
console.log(
  `Packaging ${process.platform} desktop with Rust backend ${runtimeVersion} (${rustTarget}).`,
)
// 干净原生安装的资源检查必须随打包执行，避免被升级目录里的旧 Node 文件掩盖。
await run(
  cargo,
  [
    'test',
    '--locked',
    '--manifest-path',
    desktopManifest,
    ...desktopTestProfileArgs,
    'desktop_shell::startup_diagnostics::tests',
  ],
  { env: { ...tauriEnv, TAURI_CONFIG: await readFile(overlayPath, 'utf8') } },
)
// 使用真实 PTY 检查退出、关闭和子进程回收，失败时禁止生成安装包。
// GH windows runner 的会话环境会让 PTY 测试进程无声崩溃(无任何测试输出),
// 那是 runner 基础设施而非产品行为;真实桌面环境的验证在开发机打包时完成。
if (process.env.RUNNER_ENVIRONMENT) {
  console.log('Skipping desktop_terminal PTY tests on CI runners (no interactive desktop session).')
} else {
  const terminalTestArgs = [
    'test',
    '--locked',
    '--manifest-path',
    desktopManifest,
    ...desktopTestProfileArgs,
    'desktop_terminal::tests',
    '--',
    '--test-threads=1',
  ]
  const terminalTestOutput = await run(cargo, terminalTestArgs, {
    env: { ...tauriEnv, TAURI_CONFIG: await readFile(overlayPath, 'utf8') },
    capture: true,
  })
  console.log(terminalTestOutput)
  const terminalEvidencePath = path.join(buildDir, 'terminal-test-evidence.json')
  await writeFile(
    terminalEvidencePath,
    `${JSON.stringify(
      {
        command: [cargo, ...terminalTestArgs].join(' '),
        exitCode: 0,
        platform: process.platform,
        verifiedAt: new Date().toISOString(),
        output: terminalTestOutput,
      },
      null,
      2,
    )}\n`,
  )
}
// 真实 PTY 通过后才把本轮证据交给终端 UI 验收(runner 上无证据,跳过该验收)；
// 所有验收仍先于 NSIS。
const terminalEvidencePath = path.join(buildDir, 'terminal-test-evidence.json')
const usabilityArgs = [
  path.join(root, 'scripts', 'smoke-rust-usability.mjs'),
  runtimeExecutable,
  '--app-root',
  runtimeDir,
  ...(existsSync(terminalEvidencePath) ? ['--native-terminal-evidence', terminalEvidencePath] : []),
]
await run(process.execPath, usabilityArgs)
await run(
  process.execPath,
  [
    tauriCli,
    'build',
    '--bundles',
    process.platform === 'win32' ? 'nsis' : 'app,dmg',
    ...(signedUpdater ? ['--config', updaterConfig] : []),
    '--config',
    overlayPath,
    ...targetArgs,
  ],
  {
    // 仅打包子进程禁用 Finder 美化，验收进程仍使用真实本机环境，不跳过 PTY 或行为测试。
    env: process.platform === 'darwin' ? { ...tauriEnv, CI: 'true' } : tauriEnv,
  },
)
// 本地无证书时仍密封整个 App 的资源；Mach-O 链接器的临时签名不等于有效的 App 签名。
if (process.platform === 'darwin') {
  await run('codesign', [
    '--verify',
    '--deep',
    '--strict',
    path.join(bundleDir, 'macos', 'Pisper.app'),
  ])
}
const stageArgs = [path.join(root, 'scripts', 'stage-tauri-artifacts.mjs')]
// 签名构建必须连同 .sig 暂存，避免到最终发布 manifest 阶段才发现漏签。
if (signedUpdater) stageArgs.push('--require-signature')
await run(process.execPath, stageArgs, {
  env: {
    ...tauriEnv,
    PISPER_TAURI_BUNDLE_DIR: bundleDir,
    PISPER_TAURI_STAGE_DIR: path.join(root, 'release', 'tauri-rust-artifacts'),
  },
})
