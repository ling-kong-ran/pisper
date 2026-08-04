import { createHash, randomUUID } from 'node:crypto'
import { access } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { join, resolve } from 'node:path'
import { AgentSandboxClient } from '@agent-sandbox/client'
import { compilePisperSandboxPolicy } from './policy-compiler.mjs'
import { createSandboxBashOperations } from './pi-bash-adapter.mjs'
import { DEFAULT_PISPER_SANDBOX_PROFILE } from './pisper-profiles.mjs'

const CREDENTIAL_ENVIRONMENT_NAME =
  /(?:^|_)(?:API_?KEY|ACCESS_?TOKEN|AUTH_?TOKEN|SECRET|PASSWORD|PASSWD|PRIVATE_?KEY)$/i

export function sandboxDaemonEnvironment(environment = process.env) {
  const result = {}
  for (const [name, value] of Object.entries(environment)) {
    if (value == null || CREDENTIAL_ENVIRONMENT_NAME.test(name)) continue
    result[name] = value
  }
  return result
}

export function defaultSandboxExecutable() {
  if (process.env.PISPER_SANDBOX_PATH) return resolve(process.env.PISPER_SANDBOX_PATH)
  const executable = process.platform === 'win32' ? 'agent-sandboxd.exe' : 'agent-sandboxd'
  const base = new URL('../../../sandbox/target/', import.meta.url)
  return [new URL(`release/${executable}`, base), new URL(`debug/${executable}`, base)].map(
    fileURLToPath,
  )
}

export class SandboxService {
  constructor({
    dataDir,
    executable,
    expectedSha256,
    clientFactory = AgentSandboxClient.spawn,
  } = {}) {
    this.dataDir = resolve(dataDir || process.cwd())
    this.executable = executable || defaultSandboxExecutable()
    this.expectedSha256 = expectedSha256 || process.env.PISPER_SANDBOX_SHA256 || ''
    this.clientFactory = clientFactory
    this.clientPromise = null
    this.contexts = new Map()
    this.tenantId = `pisper-${createHash('sha256').update(this.dataDir).digest('hex').slice(0, 24)}`
  }

  async resolveExecutable() {
    const candidates = Array.isArray(this.executable) ? this.executable : [this.executable]
    for (const candidate of candidates) {
      const path = resolve(String(candidate || ''))
      if (
        await access(path).then(
          () => true,
          () => false,
        )
      )
        return path
    }
    throw new Error(
      'Agent Sandbox Runtime is unavailable. Set PISPER_SANDBOX_PATH to a verified agent-sandboxd binary.',
    )
  }

  async client() {
    if (!this.clientPromise) {
      this.clientPromise = this.resolveExecutable()
        .then((executable) =>
          this.clientFactory({
            executable,
            expectedSha256: this.expectedSha256 || undefined,
            env: {
              ...sandboxDaemonEnvironment(),
              AGENT_SANDBOX_STATE_DIR: join(this.dataDir, 'sandbox-state'),
            },
            client: { name: 'pisper', version: '0.4.12' },
          }),
        )
        .catch((error) => {
          this.clientPromise = null
          throw error
        })
    }
    return this.clientPromise
  }

  async ensureContext({ contextId, cwd, protectedRoots = [], network = 'deny' }) {
    const id = String(contextId || '').trim()
    if (!id) throw new Error('Sandbox context ID is required.')
    const workspace = resolve(cwd)
    const current = this.contexts.get(id)
    if (current?.cwd === workspace) return current.promise
    await this.closeContext(id)

    const promise = this.client().then((client) =>
      client.createSandbox({
        tenantId: this.tenantId,
        profile: process.env.PISPER_SANDBOX_PROFILE || DEFAULT_PISPER_SANDBOX_PROFILE,
        authorizationId: randomUUID(),
        policy: compilePisperSandboxPolicy({
          cwd: workspace,
          protectedRoots: [this.dataDir, ...protectedRoots],
          network,
        }),
        metadata: { subject: 'agent', subjectId: id },
      }),
    )
    const record = { cwd: workspace, promise }
    this.contexts.set(id, record)
    try {
      return await promise
    } catch (error) {
      if (this.contexts.get(id) === record) this.contexts.delete(id)
      throw new Error(
        `Workspace sandbox could not be created; host fallback is disabled: ${error.message}`,
        {
          cause: error,
        },
      )
    }
  }

  createBashOperations(input) {
    return createSandboxBashOperations({
      workspace: resolve(input.cwd),
      getSandbox: () => this.ensureContext(input),
    })
  }

  async closeContext(contextId) {
    const id = String(contextId || '').trim()
    const record = this.contexts.get(id)
    if (!record) return false
    this.contexts.delete(id)
    const sandbox = await record.promise.catch(() => null)
    await sandbox?.close()
    return true
  }

  async close() {
    const results = await Promise.allSettled(
      [...this.contexts.keys()].map((id) => this.closeContext(id)),
    )
    const failure = results.find((result) => result.status === 'rejected')
    const client = await this.clientPromise?.catch(() => null)
    this.clientPromise = null
    await client?.close()
    if (failure?.status === 'rejected') throw failure.reason
  }
}
