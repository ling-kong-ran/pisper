import { existsSync } from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import {
  ProjectTrustStore,
  hasTrustRequiringProjectResources,
} from '../runtime/pi-coding-agent.mjs'

const PI_PROJECT_RESOURCES = [
  ['settings', 'settings.json'],
  ['extensions', 'extensions'],
  ['skills', 'skills'],
  ['prompts', 'prompts'],
  ['themes', 'themes'],
  ['systemPrompt', 'SYSTEM.md'],
  ['systemPrompt', 'APPEND_SYSTEM.md'],
]

const PISPER_PROJECT_RESOURCES = [
  ['skills', 'skills'],
  ['extensions', 'extensions'],
  ['prompts', 'prompts'],
]

function pathKey(value) {
  const path = resolve(value)
  return process.platform === 'win32' ? path.toLowerCase() : path
}

function projectResourceKinds(cwd) {
  const root = resolve(cwd)
  const kinds = new Set()
  for (const [kind, entry] of PI_PROJECT_RESOURCES) {
    if (existsSync(join(root, '.pi', entry))) kinds.add(kind)
  }
  for (const [kind, entry] of PISPER_PROJECT_RESOURCES) {
    if (existsSync(join(root, '.pisper', entry))) kinds.add(kind)
  }

  const userSkills = pathKey(join(homedir(), '.agents', 'skills'))
  let current = root
  while (true) {
    const skills = join(current, '.agents', 'skills')
    if (pathKey(skills) !== userSkills && existsSync(skills)) kinds.add('skills')
    const parent = dirname(current)
    if (parent === current) break
    current = parent
  }
  if (!kinds.size && hasTrustRequiringProjectResources(root)) kinds.add('projectResources')
  return [...kinds].sort()
}

export class WorkspaceTrustService {
  constructor({ agentDir } = {}) {
    this.store = new ProjectTrustStore(agentDir)
  }

  isTrusted(cwd) {
    return this.store.get(cwd) === true
  }

  getStatus(cwd) {
    const path = resolve(cwd)
    const entry = this.store.getEntry(path)
    const resources = projectResourceKinds(path)
    const decision = entry?.decision ?? null
    return {
      cwd: path,
      decision,
      trusted: decision === true,
      restricted: resources.length > 0 && decision !== true,
      requiresDecision: resources.length > 0 && decision === null,
      decisionPath: entry?.path || '',
      inherited: Boolean(entry && pathKey(entry.path) !== pathKey(path)),
      resources,
    }
  }

  setTrusted(cwd, trusted) {
    if (typeof trusted !== 'boolean') throw new Error('工作区信任决策必须是布尔值。')
    this.store.set(cwd, trusted)
    return this.getStatus(cwd)
  }
}
