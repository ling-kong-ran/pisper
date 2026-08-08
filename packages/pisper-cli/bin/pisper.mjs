#!/usr/bin/env node

import { spawn } from 'node:child_process'
import process from 'node:process'
import { ensurePisperInstallation } from '../lib/install.mjs'

try {
  const installation = await ensurePisperInstallation()
  const child = spawn(installation.executable, process.argv.slice(2), {
    env: {
      ...process.env,
      PISPER_APP_ROOT: installation.appRoot,
      PISPER_DISTRIBUTION: 'npm',
      PISPER_SIDECAR_PATH: installation.sidecar,
    },
    stdio: 'inherit',
    windowsHide: false,
  })

  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
    process.on(signal, () => {
      if (!child.killed) child.kill(signal)
    })
  }

  child.once('error', (error) => {
    console.error(`pisper: failed to start the terminal client: ${error.message}`)
    process.exitCode = 1
  })
  child.once('exit', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal)
      return
    }
    process.exitCode = code ?? 1
  })
} catch (error) {
  console.error(`pisper: ${error instanceof Error ? error.message : String(error)}`)
  process.exitCode = 1
}
