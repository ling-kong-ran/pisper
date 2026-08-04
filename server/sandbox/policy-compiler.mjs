import { resolve } from 'node:path'
import {
  PISPER_SANDBOX_INHERITED_ENVIRONMENT,
  PISPER_SANDBOX_LIMITS,
  PISPER_SANDBOX_SETTABLE_ENVIRONMENT,
} from './pisper-profiles.mjs'

export function compilePisperSandboxPolicy({ cwd, protectedRoots = [], network = 'deny' } = {}) {
  const workspace = resolve(String(cwd || ''))
  if (!cwd || !workspace) throw new Error('Sandbox workspace is required.')
  const roots = [
    ...new Set(
      protectedRoots
        .map((path) => String(path || '').trim())
        .filter(Boolean)
        .map((path) => resolve(path)),
    ),
  ]
  return {
    schemaVersion: 1,
    backend: 'native',
    filesystem: {
      mounts: [{ name: 'workspace', source: workspace, access: 'read-write' }],
      protectedRoots: roots,
      tempBytes: 2 * 1024 * 1024 * 1024,
    },
    network: { mode: network === 'host' ? 'host' : 'deny' },
    environment: {
      inherit: [...PISPER_SANDBOX_INHERITED_ENVIRONMENT],
      allowSet: [...PISPER_SANDBOX_SETTABLE_ENVIRONMENT],
    },
    limits: { ...PISPER_SANDBOX_LIMITS },
  }
}
