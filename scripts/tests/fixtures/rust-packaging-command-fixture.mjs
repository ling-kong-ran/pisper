import childProcess from 'node:child_process'
import { EventEmitter } from 'node:events'
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
process.on('exit', () => {
  process.stdout.write(`\nRUST_PACKAGING_TEST_COMMANDS=${JSON.stringify(commands)}\n`)
})

childProcess.spawn = (command, args, options) => {
  commands.push({
    command,
    args,
    ci: options.env.CI,
    deploymentTarget: options.env.MACOSX_DEPLOYMENT_TARGET,
    keyPresent: Boolean(options.env.TAURI_SIGNING_PRIVATE_KEY?.trim()),
    passwordPresent: Boolean(options.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD?.trim()),
    bundleDir: options.env.PISPER_TAURI_BUNDLE_DIR,
    stageDir: options.env.PISPER_TAURI_STAGE_DIR,
  })
  let stdout = ''
  if (command === 'fixture-rustc' && args.join(' ') === '-vV') {
    stdout = `host: ${process.env.PISPER_PACKAGING_TEST_TARGET}\n`
  } else if (command === 'git' && args.join(' ') === 'rev-parse HEAD') {
    stdout = '1111111111111111111111111111111111111111\n'
  } else if (command === 'git' && args.join(' ') === 'status --porcelain --untracked-files=no') {
    stdout = ''
  } else if (command === 'fixture-cargo' && ['test', 'build', 'rustc'].includes(args[0])) {
    stdout = 'test result: ok. synthetic command boundary\n'
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
    child.emit('close', 0, null)
  })
  return child
}
syncBuiltinESMExports()
