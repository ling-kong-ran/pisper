/**
 * Stateful Anthropic SRT lifecycle adapter.
 *
 * Security decisions live in ../security/local-sandbox-policy.mjs; this service
 * owns installation, initialization, workspace scheduling, process trees,
 * status reporting, failure recovery, and shutdown cleanup.
 */
import { execFileSync, spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'
import { resolveAgentDataDir } from '../data-dir-migration.mjs'
import {
  DEFAULT_WINDOWS_PROXY_PORT_RANGE,
  SandboxManager,
  VENDORED_SRT_WIN_EXE,
  WindowsSandboxError,
  checkWindowsSandboxStatusAsync,
  installWindowsSandboxAsync,
  resolveSrtWin,
} from '@anthropic-ai/sandbox-runtime'
import {
  buildCommandSandboxOverrides,
  buildSandboxConfig,
  buildWindowsSandboxCommand,
  canonicalSandboxPath,
  packagedSandboxExecutablePath,
  sandboxChildEnvironment,
  windowsDenyReadPaths,
  windowsSensitiveGuardPaths,
} from '../security/local-sandbox-policy.mjs'

export const SANDBOX_BACKEND = 'anthropic-srt'

function sandboxErrorState(error) {
  const code = error instanceof WindowsSandboxError ? error.code : ''
  if (code === 'not_provisioned') return 'not-installed'
  if (code === 'srt_win_not_found') return 'unsupported'
  return 'error'
}

function publicSandboxError(error) {
  return {
    state: sandboxErrorState(error),
    code: error instanceof WindowsSandboxError ? error.code : 'sandbox-initialization-failed',
    message: error instanceof Error ? error.message : String(error),
  }
}

function resolveWindowsShell() {
  const configured = String(process.env.PISPER_SANDBOX_SHELL || '').trim()
  if (configured && existsSync(configured)) return configured
  try {
    const matches = execFileSync('where.exe', ['bash'], { encoding: 'utf8', windowsHide: true })
      .split(/\r?\n/)
      .map((value) => value.trim())
      .filter(Boolean)
    const gitBash = matches.find((value) => /\\Git\\(?:usr\\bin|bin)\\bash\.exe$/i.test(value))
    if (gitBash) return gitBash
  } catch {}
  const programFiles = process.env.ProgramFiles || 'C:\\Program Files'
  const candidates = [
    join(programFiles, 'Git', 'usr', 'bin', 'bash.exe'),
    join(programFiles, 'Git', 'bin', 'bash.exe'),
  ]
  return candidates.find(existsSync) || 'powershell'
}

function abortedError() {
  return new Error('aborted')
}

function unrefDelay(milliseconds) {
  return new Promise((resolveDelay) => {
    const handle = setTimeout(resolveDelay, milliseconds)
    handle.unref?.()
  })
}

function terminateProcessTree(child, platform = process.platform) {
  if (!child?.pid || child.exitCode != null || child.signalCode != null) return
  if (platform === 'win32') {
    try {
      spawn('taskkill.exe', ['/pid', String(child.pid), '/t', '/f'], { windowsHide: true, stdio: 'ignore' }).unref()
    } catch {
      child.kill('SIGKILL')
    }
    return
  }
  try {
    process.kill(-child.pid, 'SIGKILL')
  } catch {
    child.kill('SIGKILL')
  }
}

export class LocalSandboxService {
  constructor({ dataDir, homeDir = homedir(), platform = process.platform, manager = SandboxManager, windowsExecutable = VENDORED_SRT_WIN_EXE, windowsShell = '', disposeGraceMs = 5_000 } = {}) {
    this.homeDir = resolve(homeDir)
    this.dataDir = resolve(dataDir || resolveAgentDataDir({ home: this.homeDir, env: {} }))
    this.platform = platform
    this.manager = manager
    this.windowsExecutable = packagedSandboxExecutablePath(windowsExecutable)
    this.windowsShell = String(windowsShell || '').trim()
    this.disposeGraceMs = disposeGraceMs
    this.workspaces = new Set()
    this.initializedSignature = ''
    this.lock = Promise.resolve()
    this.activeCommands = 0
    this.pendingCommands = 0
    this.activeChildren = new Set()
    this.idleWaiters = []
    this.switchingWorkspace = false
    this.lastSwitchDurationMs = null
    this.disposing = false
    this.disposed = false
    this.lastError = null
  }

  isSupported() {
    return ['win32', 'darwin', 'linux'].includes(this.platform) && this.manager.isSupportedPlatform()
  }

  windowsSpawn() {
    return resolveSrtWin({ path: this.windowsExecutable })
  }

  registerWorkspace(cwd) {
    if (!cwd) return
    const workspace = canonicalSandboxPath(cwd)
    if (this.platform === 'win32') {
      // The Windows backend grants ACLs to one shared sandbox account. Keeping
      // multiple roots granted concurrently would let one session modify
      // another session's workspace, while denying the other roots per command
      // reintroduces the expensive ACL stamp path. Serialize workspace switches
      // instead and initialize SRT with one root at a time.
      this.workspaces = new Set([workspace])
      return
    }
    this.workspaces.add(workspace)
  }

  config() {
    return buildSandboxConfig({
      workspaces: this.workspaces,
      dataDir: this.dataDir,
      platform: this.platform,
      srtWinPath: this.windowsExecutable,
      homeDir: this.homeDir,
    })
  }

  signature() {
    return JSON.stringify({
      workspaces: [...this.workspaces].sort(),
      windowsDenyRead: this.platform === 'win32' ? windowsDenyReadPaths(this.dataDir, this.homeDir).sort() : [],
    })
  }

  async withLock(run) {
    const previous = this.lock
    let release
    this.lock = new Promise((resolveLock) => { release = resolveLock })
    await previous
    try {
      return await run()
    } finally {
      release()
    }
  }

  waitForIdle(signal) {
    if (this.activeCommands === 0) return Promise.resolve()
    if (signal?.aborted) return Promise.reject(abortedError())
    return new Promise((resolveIdle, rejectIdle) => {
      const waiter = {
        resolve: () => {
          signal?.removeEventListener('abort', waiter.abort)
          resolveIdle()
        },
        abort: () => {
          const index = this.idleWaiters.indexOf(waiter)
          if (index >= 0) this.idleWaiters.splice(index, 1)
          rejectIdle(abortedError())
        },
      }
      signal?.addEventListener('abort', waiter.abort, { once: true })
      this.idleWaiters.push(waiter)
    })
  }

  releaseCommand() {
    this.activeCommands = Math.max(0, this.activeCommands - 1)
    if (this.activeCommands !== 0) return
    for (const waiter of this.idleWaiters.splice(0)) waiter.resolve()
  }

  schedulerStatus() {
    return {
      activeCommands: this.activeCommands,
      pendingCommands: this.pendingCommands,
      switchingWorkspace: this.switchingWorkspace,
      lastSwitchDurationMs: this.lastSwitchDurationMs,
    }
  }

  async acquire(cwd, signal) {
    if (this.disposing || this.disposed) throw new Error('本地沙箱正在关闭。')
    if (signal?.aborted) throw abortedError()
    this.pendingCommands += 1
    try {
      return await this.withLock(async () => {
        if (this.disposing || this.disposed) throw new Error('本地沙箱正在关闭。')
        if (signal?.aborted) throw abortedError()
        const previousWorkspaces = new Set(this.workspaces)
        this.registerWorkspace(cwd)
        let signature
        try {
          signature = this.signature()
        } catch (error) {
          this.workspaces = previousWorkspaces
          this.lastError = publicSandboxError(error)
          throw error
        }
        const switching = signature !== this.initializedSignature
        const switchStartedAt = switching ? Date.now() : null
        if (switching) this.switchingWorkspace = true
        try {
          if (switching) {
            await this.waitForIdle(signal)
            if (this.disposing || this.disposed) throw new Error('本地沙箱正在关闭。')
            if (signal?.aborted) throw abortedError()
            if (this.manager.isSandboxingEnabled()) await this.manager.reset()
            this.initializedSignature = ''
          }
          if (!this.initializedSignature) {
            if (!this.isSupported()) throw new Error(`当前平台 ${this.platform} 不支持本地沙箱。`)
            await this.manager.initialize(this.config(), undefined, false)
            this.initializedSignature = signature
          }
          this.lastError = null
        } catch (error) {
          this.workspaces = previousWorkspaces
          this.lastError = publicSandboxError(error)
          throw error
        } finally {
          if (switchStartedAt != null) {
            this.switchingWorkspace = false
            this.lastSwitchDurationMs = Date.now() - switchStartedAt
          }
        }
        if (this.disposing || this.disposed) throw new Error('本地沙箱正在关闭。')
        if (signal?.aborted) throw abortedError()
        this.activeCommands += 1
        return () => this.releaseCommand()
      })
    } finally {
      this.pendingCommands = Math.max(0, this.pendingCommands - 1)
    }
  }

  createBashOperations() {
    return {
      exec: async (command, cwd, { onData, signal, timeout }) => {
        const release = await this.acquire(cwd, signal)
        let child
        let timedOut = false
        let timeoutHandle
        const shellPath = this.platform === 'win32' ? (this.windowsShell || resolveWindowsShell()) : '/bin/bash'
        try {
          if (this.disposing || this.disposed) throw new Error('本地沙箱正在关闭。')
          const wrapped = await this.manager.wrapWithSandboxArgv(
            this.platform === 'win32' ? buildWindowsSandboxCommand(command, shellPath, windowsSensitiveGuardPaths(this.dataDir, this.homeDir)) : command,
            shellPath,
            buildCommandSandboxOverrides(this.workspaces, cwd, this.platform),
            signal,
            cwd,
          )
          if (this.disposing || this.disposed) throw new Error('本地沙箱正在关闭。')
          if (signal?.aborted) throw abortedError()
          child = spawn(wrapped.argv[0], wrapped.argv.slice(1), {
            cwd,
            detached: this.platform !== 'win32',
            windowsHide: true,
            stdio: ['ignore', 'pipe', 'pipe'],
            // The sandbox wrapper owns the final environment. In particular, it
            // removes denied credentials and injects the network proxy boundary.
            env: sandboxChildEnvironment(wrapped.env),
          })
          if (this.disposing || this.disposed) {
            terminateProcessTree(child, this.platform)
            throw new Error('本地沙箱正在关闭。')
          }
          this.activeChildren.add(child)
          if (timeout && timeout > 0) {
            timeoutHandle = setTimeout(() => {
              timedOut = true
              terminateProcessTree(child, this.platform)
            }, timeout * 1000)
            timeoutHandle.unref?.()
          }
          const abort = () => terminateProcessTree(child, this.platform)
          if (signal?.aborted) abort()
          else signal?.addEventListener('abort', abort, { once: true })
          return await new Promise((resolveExec, rejectExec) => {
            child.stdout?.on('data', onData)
            child.stderr?.on('data', onData)
            child.on('error', rejectExec)
            child.on('close', (code) => {
              signal?.removeEventListener('abort', abort)
              if (signal?.aborted) rejectExec(new Error('aborted'))
              else if (timedOut) rejectExec(new Error(`timeout:${timeout}`))
              else resolveExec({ exitCode: code })
            })
          })
        } finally {
          if (timeoutHandle) clearTimeout(timeoutHandle)
          if (child) this.activeChildren.delete(child)
          try { this.manager.cleanupAfterCommand() } catch {}
          release()
        }
      },
    }
  }

  async getStatus() {
    if (!this.isSupported()) return { supported: false, backend: SANDBOX_BACKEND, platform: this.platform, state: 'unsupported' }
    try {
      if (this.platform === 'win32') {
        const status = await checkWindowsSandboxStatusAsync({ srtWin: this.windowsSpawn() })
        const ready = Boolean(status.user.provisioned && status.user.credPresent)
        return {
          supported: true,
          backend: SANDBOX_BACKEND,
          platform: this.platform,
          state: ready ? (this.activeCommands > 0 ? 'active' : 'ready') : 'not-installed',
          requiresElevation: !ready,
          initialized: Boolean(this.initializedSignature),
          userProvisioned: Boolean(status.user.provisioned),
          networkFilter: status.wfp.state,
          scheduler: this.schedulerStatus(),
          error: this.lastError,
        }
      }
      const dependencies = await this.manager.checkDependenciesAsync()
      return {
        supported: dependencies.errors.length === 0,
        backend: SANDBOX_BACKEND,
        platform: this.platform,
        state: dependencies.errors.length ? 'unavailable' : this.activeCommands > 0 ? 'active' : 'ready',
        requiresElevation: false,
        initialized: Boolean(this.initializedSignature),
        scheduler: this.schedulerStatus(),
        errors: dependencies.errors,
        warnings: dependencies.warnings,
        error: this.lastError,
      }
    } catch (error) {
      return {
        supported: false,
        backend: SANDBOX_BACKEND,
        platform: this.platform,
        requiresElevation: false,
        scheduler: this.schedulerStatus(),
        ...publicSandboxError(error),
      }
    }
  }

  async install() {
    if (this.platform !== 'win32') return this.getStatus()
    return this.withLock(async () => {
      try {
        if (this.disposing || this.disposed) throw new Error('本地沙箱正在关闭。')
        await this.waitForIdle()
        if (this.manager.isSandboxingEnabled()) await this.manager.reset()
        this.initializedSignature = ''
        const result = await installWindowsSandboxAsync({
          srtWin: this.windowsSpawn(),
          proxyPortRange: DEFAULT_WINDOWS_PROXY_PORT_RANGE,
        })
        if (result.cancelled) return { ...(await this.getStatus()), cancelled: true }
        this.lastError = null
        return this.getStatus()
      } catch (error) {
        this.lastError = publicSandboxError(error)
        throw error
      }
    })
  }

  async dispose() {
    if (this.disposed) return
    this.disposing = true
    for (const child of this.activeChildren) terminateProcessTree(child, this.platform)
    await this.withLock(async () => {
      for (const child of this.activeChildren) terminateProcessTree(child, this.platform)
      // Briefly wait for tracked children to exit, but never block dispose for the full grace
      // period when kill is slow or a command is still between acquire() and spawn().
      if (this.activeCommands > 0 && this.activeChildren.size > 0) {
        await Promise.race([
          this.waitForIdle(),
          unrefDelay(Math.min(this.disposeGraceMs, 250)),
        ])
      }
      for (const child of [...this.activeChildren]) {
        terminateProcessTree(child, this.platform)
        this.activeChildren.delete(child)
      }
      if (this.activeCommands > 0) {
        this.activeCommands = 0
        for (const waiter of this.idleWaiters.splice(0)) waiter.resolve()
      }
      if (this.manager.isSandboxingEnabled()) {
        try { await this.manager.reset() } catch (error) { this.lastError = publicSandboxError(error) }
      }
      this.initializedSignature = ''
      this.workspaces.clear()
      this.disposed = true
      this.disposing = false
    })
  }
}
