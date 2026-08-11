import { spawn } from 'node:child_process'
import process from 'node:process'

const child = spawn(
  process.execPath,
  ['--import', 'tsx', '--test', 'runtime/tests/sidecar.test.mjs'],
  {
    env: { ...process.env, PISPER_STARTUP_GATE: '1' },
    stdio: 'inherit',
    windowsHide: true,
  },
)

child.once('error', (error) => {
  console.error(error)
  process.exitCode = 1
})
child.once('exit', (code, signal) => {
  if (signal) {
    console.error(`Sidecar startup gate terminated by ${signal}.`)
    process.exitCode = 1
  } else {
    process.exitCode = code ?? 1
  }
})
