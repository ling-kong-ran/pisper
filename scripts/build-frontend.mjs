import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { dirname, resolve } from 'node:path'

const require = createRequire(import.meta.url)
const viteCli = resolve(dirname(require.resolve('vite')), '../../bin/vite.js')
const child = spawn(process.execPath, [viteCli, 'build'], {
  env: { ...process.env, NODE_ENV: 'production' },
  stdio: 'inherit',
})

child.on('error', (error) => {
  console.error(`Failed to start Vite: ${error.message}`)
  process.exitCode = 1
})

child.on('exit', (code, signal) => {
  if (signal) {
    console.error(`Vite terminated by ${signal}`)
    process.exitCode = 1
    return
  }
  process.exitCode = code ?? 1
})
