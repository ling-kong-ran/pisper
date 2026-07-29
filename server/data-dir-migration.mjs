import { homedir } from 'node:os'
import { join, resolve } from 'node:path'

export const CURRENT_HOME = '.pisper'

export function resolveAgentDataDir({
  env = process.env,
  home = homedir(),
} = {}) {
  const explicit = env.PISPER_AGENT_DIR
  return explicit ? resolve(explicit) : join(home, CURRENT_HOME, 'agent')
}
