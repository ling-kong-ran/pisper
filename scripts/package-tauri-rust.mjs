import { spawn } from 'node:child_process'
import { copyFile, mkdir, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { existsSync } from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { stageRustRuntime } from './stage-rust-runtime.mjs'
import { resolveRustDesktopPlatform } from './rust-desktop-platforms.mjs'

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

// 先拒绝未知平台，不能把缺少原生资源的平台误当成可构建。
resolveRustDesktopPlatform()

function run(command, args, { env = process.env, capture = false, input, timeoutMs } = {}) {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, args, {
      cwd: root,
      env,
      stdio: capture ? [input === undefined ? 'ignore' : 'pipe', 'pipe', 'pipe'] : 'inherit',
    })
    const timer = timeoutMs ? setTimeout(() => child.kill('SIGKILL'), timeoutMs) : undefined
    let stdout = ''
    let stderr = ''
    if (capture) {
      child.stdout.setEncoding('utf8').on('data', (chunk) => (stdout += chunk))
      child.stderr.setEncoding('utf8').on('data', (chunk) => (stderr += chunk))
    }
    child.once('error', (error) => {
      clearTimeout(timer)
      rejectRun(error)
    })
    if (input !== undefined) {
      child.stdin.once('error', rejectRun)
      child.stdin.end(input)
    }
    child.once('close', (code, signal) => {
      clearTimeout(timer)
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
const tuiVersion = desktopPackage.bundled?.tui
if (typeof tuiVersion !== 'string' || !/^\d+\.\d+\.\d+$/.test(tuiVersion))
  throw new Error('desktop-package.json must declare bundled.tui as X.Y.Z before packaging.')
if (typeof runtimeVersion !== 'string' || !runtimeVersion.trim()) {
  throw new Error('desktop-package.json must declare bundled.runtime before packaging.')
}
const rustVersion = await run(process.env.RUSTC || 'rustc', ['-vV'], { capture: true })
const rustTarget = rustVersion.match(/^host:\s*(\S+)$/m)?.[1]
if (!rustTarget) throw new Error('Unable to resolve the Rust host target from rustc -vV.')
const desktop = resolveRustDesktopPlatform({ rustTarget })
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
if (desktop.minimumSystemVersion) {
  // 运行时、CLI 和桌面壳的最低系统必须与打包原生依赖一致。
  for (const env of [runtimeEnv, tuiEnv, tauriEnv])
    env.MACOSX_DEPLOYMENT_TARGET = desktop.minimumSystemVersion
}
for (const name of [
  'APPLE_API_ISSUER',
  'APPLE_API_KEY',
  'APPLE_API_KEY_PATH',
  'APPLE_CERTIFICATE',
  'APPLE_CERTIFICATE_PASSWORD',
  'APPLE_SIGNING_IDENTITY',
  'TAURI_SIGNING_PRIVATE_KEY',
  'TAURI_SIGNING_PRIVATE_KEY_PASSWORD',
]) {
  if (!tauriEnv[name]?.trim()) delete tauriEnv[name]
}
// 保留旧桌面入口从忽略目录读取本机更新签名凭据的行为，不输出密钥。
if (!tauriEnv.TAURI_SIGNING_PRIVATE_KEY) {
  for (const [name, file] of [
    ['TAURI_SIGNING_PRIVATE_KEY', 'tauri-updater.key'],
    ['TAURI_SIGNING_PRIVATE_KEY_PASSWORD', 'tauri-updater-key.password'],
  ]) {
    try {
      const value = (await readFile(path.join(root, 'release', file), 'utf8')).trim()
      if (value) tauriEnv[name] = value
    } catch (error) {
      if (error.code !== 'ENOENT') throw error
    }
  }
}
const signedUpdater = Boolean(tauriEnv.TAURI_SIGNING_PRIVATE_KEY?.trim())

// frozen source 的 Cargo 版本可能早于同批次已发布的 TUI；暂存只属于本次编译。
// 无论构建、CLI 验证或写入在哪一步失败，都恢复原字节，不混入 Desktop 版本提交。
async function buildDesktopTui() {
  const lockPath = path.join(root, 'src-tui', 'Cargo.lock')
  const [originalManifest, originalLock] = await Promise.all([
    readFile(tuiManifest),
    readFile(lockPath),
  ])
  const manifestText = originalManifest.toString('utf8')
  const lockText = originalLock.toString('utf8')
  const manifestVersion = /(\[package\][\s\S]*?\r?\nversion\s*=\s*")[^"]+("[ \t]*(?:\r?\n|$))/
  const lockVersion =
    /(\[\[package\]\]\s*\r?\nname\s*=\s*"pisper-tui"\s*\r?\nversion\s*=\s*")[^"]+("[ \t]*(?:\r?\n|$))/
  if (!manifestVersion.test(manifestText) || !lockVersion.test(lockText))
    throw new Error('Unable to stage the bundled TUI Cargo version.')
  const failures = []
  try {
    await writeFile(tuiManifest, manifestText.replace(manifestVersion, `$1${tuiVersion}$2`))
    await writeFile(lockPath, lockText.replace(lockVersion, `$1${tuiVersion}$2`))
    await run(cargo, ['build', '--release', '--locked', '--manifest-path', tuiManifest], {
      env: tuiEnv,
    })
    const cli = path.join(root, 'src-tui', 'target', 'release', `pisper${executableSuffix}`)
    const actual = await run(cli, ['--version'], { env: tuiEnv, capture: true, timeoutMs: 10000 })
    if (actual !== `pisper ${tuiVersion}`)
      throw new Error(`Bundled CLI version mismatch: expected pisper ${tuiVersion}, got ${actual}.`)
  } catch (error) {
    failures.push(error)
  } finally {
    const restored = await Promise.allSettled([
      writeFile(tuiManifest, originalManifest),
      writeFile(lockPath, originalLock),
    ])
    for (const result of restored) if (result.status === 'rejected') failures.push(result.reason)
  }
  if (failures.length === 1) throw failures[0]
  if (failures.length)
    throw new AggregateError(
      failures,
      'Unable to build bundled CLI or restore original TUI Cargo files.',
    )
}
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
const terminalEvidencePath = path.join(buildDir, 'terminal-test-evidence.json')
await rm(terminalEvidencePath, { force: true })
// Windows 不覆盖可能运行中的默认 exe；macOS 使用 Cargo 的标准命名，
// 避免 rustc -o 在多 emit 类型下附加 crate hash 后找不到暂存入口。
if (process.platform !== 'win32') {
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
      '--bin',
      'pisper-server',
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
// 桌面与独立 CLI 使用同一套资源和审计元数据。
await stageRustRuntime({
  root,
  runtimeDir,
  runtimeExecutable,
  runtimeVersion,
  sourceCommit,
  sourceDirty: Boolean(
    await run('git', ['status', '--porcelain', '--untracked-files=no'], { capture: true }),
  ),
})
await run(process.execPath, [
  path.join(root, 'scripts/verify-rust-runtime-resources.mjs'),
  runtimeExecutable,
  runtimeDir,
])
await buildDesktopTui()

await mkdir(binariesDir, { recursive: true })
await copyFile(
  runtimeExecutable,
  path.join(binariesDir, `pisper-sidecar-${rustTarget}${executableSuffix}`),
)
await copyFile(
  path.join(root, 'src-tui', 'target', 'release', `pisper${executableSuffix}`),
  path.join(binariesDir, `pisper-cli-${rustTarget}${executableSuffix}`),
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
                minimumSystemVersion: desktop.minimumSystemVersion,
                signingIdentity: tauriEnv.APPLE_SIGNING_IDENTITY || '-',
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

const targetArgs = desktop.targetArgs
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
if (process.platform === 'win32' && process.env.RUNNER_ENVIRONMENT) {
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
const usabilityArgs = [
  path.join(root, 'scripts', 'smoke-rust-usability.mjs'),
  runtimeExecutable,
  '--app-root',
  runtimeDir,
  ...(existsSync(terminalEvidencePath) ? ['--native-terminal-evidence', terminalEvidencePath] : []),
]
await run(process.execPath, usabilityArgs)
const buildArgs = [
  tauriCli,
  'build',
  '--bundles',
  desktop.bundles,
  ...(signedUpdater ? ['--config', updaterConfig] : []),
  '--config',
  overlayPath,
  ...targetArgs,
]
await rm(bundleDir, { recursive: true, force: true })
await run(process.execPath, buildArgs, {
  // 仅打包子进程禁用 Finder 美化，验收进程仍使用真实本机环境，不跳过 PTY 或行为测试。
  env: process.platform === 'darwin' ? { ...tauriEnv, CI: 'true' } : tauriEnv,
})
// 本地无证书时仍密封整个 App 的资源；Mach-O 链接器的临时签名不等于有效的 App 签名。
if (process.platform === 'darwin') {
  await run('codesign', [
    '--verify',
    '--deep',
    '--strict',
    path.join(bundleDir, 'macos', 'Pisper.app'),
  ])
}
if (desktop.simpleIntelMacDmg) {
  // Intel CI 的 Finder 美化会卡住；从已签名 App 直接生成 DMG，保留原资源签名。
  const sourceDir = path.join(bundleDir, 'dmg-source')
  const dmgDir = path.join(bundleDir, 'dmg')
  await mkdir(sourceDir, { recursive: true })
  await mkdir(dmgDir, { recursive: true })
  await run('ditto', [
    path.join(bundleDir, 'macos', 'Pisper.app'),
    path.join(sourceDir, 'Pisper.app'),
  ])
  await symlink('/Applications', path.join(sourceDir, 'Applications'))
  await run('hdiutil', [
    'create',
    '-volname',
    'Pisper',
    '-srcfolder',
    sourceDir,
    '-ov',
    '-format',
    'UDZO',
    path.join(dmgDir, `Pisper_${desktopPackage.version}_x64.dmg`),
  ])
}
const stageArgs = [path.join(root, 'scripts', 'stage-tauri-artifacts.mjs')]
// 签名构建必须连同 .sig 暂存，避免到最终发布 manifest 阶段才发现漏签。
if (signedUpdater) stageArgs.push('--require-signature')
const stageEnv = {
  ...tauriEnv,
  PISPER_TAURI_BUNDLE_DIR: bundleDir,
  PISPER_TAURI_STAGE_DIR: path.join(root, 'release', 'tauri-rust-artifacts'),
}
await run(process.execPath, stageArgs, { env: stageEnv })
if (desktop.windowsOffline) {
  // 普通包先归档；清除同名 NSIS 输出后再构建离线包，不能把旧包混入本次发布。
  await rm(bundleDir, { recursive: true, force: true })
  await run(
    process.execPath,
    [...buildArgs, '--config', path.join(root, 'src-tauri', 'tauri.windows-offline.conf.json')],
    { env: tauriEnv },
  )
  await run(process.execPath, [...stageArgs, '--windows-offline'], { env: stageEnv })
}
