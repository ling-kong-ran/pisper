#!/usr/bin/env node

import { spawn } from 'node:child_process'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { ensurePisperInstallation } from '../lib/install.mjs'
import { handleNpmHelp, handleNpmUpdate } from '../lib/npm-update.mjs'

const frontendRoot = fileURLToPath(new URL('../web', import.meta.url))

try {
  const arguments_ = process.argv.slice(2)
  if (!handleNpmHelp(arguments_) && !(await handleNpmUpdate(arguments_))) {
    const installation = await ensurePisperInstallation()
    const env = {
      ...process.env,
      PISPER_APP_ROOT: installation.appRoot,
      PISPER_DISTRIBUTION: 'npm',
      PISPER_FRONTEND_ROOT: frontendRoot,
    }
    if (installation.runtimeBackend === 'rust') {
      // npm 配对的原生 Runtime 必须优先于用户遗留的 Node 启动配置。
      delete env.PISPER_RUNTIME_NODE
      env.PISPER_SIDECAR_PATH = installation.sidecar
    } else {
      delete env.PISPER_SIDECAR_PATH
      env.PISPER_RUNTIME_NODE = process.execPath
    }
    const child = spawn(installation.executable, arguments_, {
      env,
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
  }
} catch (error) {
  console.error(`pisper: ${error instanceof Error ? error.message : String(error)}`)
  process.exitCode = 1
}
