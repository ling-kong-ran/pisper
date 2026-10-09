import childProcess from 'node:child_process'
import { EventEmitter } from 'node:events'
import { syncBuiltinESMExports } from 'node:module'
import path from 'node:path'
import { PassThrough } from 'node:stream'

// Linux quality job 也运行 Windows 入口；子进程边界只记录，不启动编译器或打包器。
Object.defineProperty(process, 'platform', { value: 'win32' })
const commands = []
process.on('exit', () => {
  process.stdout.write(`\nRUST_PACKAGING_TEST_COMMANDS=${JSON.stringify(commands)}\n`)
})

childProcess.spawn = (command, args, options) => {
  commands.push({
    command,
    args,
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
  } else if (
    command !== process.execPath ||
    ![
      'build-frontend.mjs',
      'check-bundle-budget.mjs',
      'check-dist-compat.mjs',
      'smoke-rust-usability.mjs',
      'tauri.js',
      'stage-tauri-artifacts.mjs',
    ].includes(path.basename(args[0]))
  ) {
    throw new Error(`Unexpected packaging command: ${command} ${args.join(' ')}`)
  }

  const child = new EventEmitter()
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
