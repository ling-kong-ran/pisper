import childProcess from 'node:child_process'
import { EventEmitter } from 'node:events'
import { existsSync, readFileSync } from 'node:fs'
import { syncBuiltinESMExports } from 'node:module'
import path from 'node:path'
import { PassThrough } from 'node:stream'

// Linux quality job 也运行 Windows 入口；子进程边界只记录，不启动编译器或打包器。
const target = process.env.PISPER_PACKAGING_TEST_TARGET
const darwin = target.endsWith('apple-darwin')
Object.defineProperty(process, 'platform', {
  value: darwin ? 'darwin' : target.includes('linux') ? 'linux' : 'win32',
})
Object.defineProperty(process, 'arch', { value: target.startsWith('aarch64') ? 'arm64' : 'x64' })
const commands = []
let builtTuiVersion
process.on('exit', () => {
  process.stdout.write(`\nRUST_PACKAGING_TEST_COMMANDS=${JSON.stringify(commands)}\n`)
})

childProcess.spawn = (command, args, options) => {
  const manifestIndex = args.indexOf('--manifest-path')
  const buildTui =
    command === 'fixture-cargo' &&
    args[0] === 'build' &&
    manifestIndex >= 0 &&
    /src-tui[\\/]Cargo\.toml$/.test(args[manifestIndex + 1])
  let lockVersion
  if (buildTui) {
    const manifest = args[manifestIndex + 1]
    builtTuiVersion = readFileSync(manifest, 'utf8').match(
      /\[package\][\s\S]*?\r?\nversion\s*=\s*"([^"]+)"/,
    )?.[1]
    const lock = path.join(path.dirname(manifest), 'Cargo.lock')
    if (existsSync(lock))
      lockVersion = readFileSync(lock, 'utf8').match(
        /name\s*=\s*"pisper-tui"\s*\r?\nversion\s*=\s*"([^"]+)"/,
      )?.[1]
  }
  commands.push({
    command,
    args,
    ...(buildTui ? { tuiManifestVersion: builtTuiVersion, tuiLockVersion: lockVersion } : {}),
    ci: options.env.CI,
    deploymentTarget: options.env.MACOSX_DEPLOYMENT_TARGET,
    keyPresent: Boolean(options.env.TAURI_SIGNING_PRIVATE_KEY?.trim()),
    passwordPresent: Boolean(options.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD?.trim()),
    // 只记录存在性与原值比较结果，避免把签名或公证凭据写入测试输出。
    signingEnvironment: Object.fromEntries(
      [
        'APPLE_ID',
        'APPLE_PASSWORD',
        'APPLE_TEAM_ID',
        'APPLE_API_KEY',
        'APPLE_API_ISSUER',
        'APPLE_API_KEY_PATH',
        'TAURI_SIGNING_PRIVATE_KEY',
        'TAURI_SIGNING_PRIVATE_KEY_PASSWORD',
      ].map((name) => [
        name,
        {
          present: Object.hasOwn(options.env, name),
          matchesInput: options.env[name] === process.env[name],
        },
      ]),
    ),
    bundleDir: options.env.PISPER_TAURI_BUNDLE_DIR,
    stageDir: options.env.PISPER_TAURI_STAGE_DIR,
  })
  let stdout = ''
  let code = 0
  if (command === 'fixture-rustc' && args.join(' ') === '-vV') {
    stdout = `host: ${process.env.PISPER_PACKAGING_TEST_TARGET}\n`
  } else if (command === 'git' && args.join(' ') === 'rev-parse HEAD') {
    stdout = '1111111111111111111111111111111111111111\n'
  } else if (command === 'git' && args.join(' ') === 'status --porcelain --untracked-files=no') {
    stdout = ''
  } else if (command === 'fixture-cargo' && ['test', 'build', 'rustc'].includes(args[0])) {
    stdout = 'test result: ok. synthetic command boundary\n'
    if (buildTui && process.env.PISPER_PACKAGING_TEST_TUI_BUILD_FAIL === '1') code = 1
  } else if (/^pisper(?:\.exe)?$/.test(path.basename(command)) && args.join(' ') === '--version') {
    stdout = `pisper ${process.env.PISPER_PACKAGING_TEST_CLI_VERSION || builtTuiVersion}\n`
  } else if (args.includes('--pisper-speech-worker')) {
    stdout = JSON.stringify({
      id: 'packaging-version',
      ok: true,
      result: { sherpa: '1.13.7', onnxruntime: darwin ? '1.28.1' : '1.27.1' },
    })
  } else if (darwin && ['ditto', 'hdiutil'].includes(command)) {
    stdout = ''
  } else if (
    command === 'codesign' &&
    darwin &&
    args.slice(0, 3).join(' ') === '--verify --deep --strict'
  ) {
    stdout = ''
  } else if (
    command !== process.execPath ||
    ![
      'build-frontend.mjs',
      'verify-rust-runtime-resources.mjs',
      'check-bundle-budget.mjs',
      'check-dist-compat.mjs',
      'smoke-rust-usability.mjs',
      'smoke-rust-provider-upgrade.mjs',
      'tauri.js',
      'stage-tauri-artifacts.mjs',
    ].includes(path.basename(args[0]))
  ) {
    throw new Error(`Unexpected packaging command: ${command} ${args.join(' ')}`)
  }

  const child = new EventEmitter()
  child.stdin = new PassThrough()
  child.stdout = new PassThrough()
  child.stderr = new PassThrough()
  queueMicrotask(() => {
    child.stdout.end(stdout)
    child.stderr.end()
    child.emit('close', code, null)
  })
  return child
}
syncBuiltinESMExports()
