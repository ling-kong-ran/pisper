import { resolve } from 'node:path'
import { ExtensionsService } from '../services/extensions-service.mjs'
import { SkillsService } from '../services/skills-service.mjs'
import { SettingsManager } from './pi-coding-agent.mjs'

function workspaceKey(path) {
  const value = resolve(path)
  return process.platform === 'win32' ? value.toLowerCase() : value
}

export function createWorkspaceResourceServices({
  agentDir,
  cwd,
  getPrimarySettings,
  isProjectTrusted,
  extensionFactories,
}) {
  const primaryKey = workspaceKey(cwd)
  const getSettingsManager = (workspaceCwd = cwd) => {
    const primarySettings = getPrimarySettings()
    if (!primarySettings || workspaceKey(workspaceCwd) === primaryKey) return primarySettings
    return SettingsManager.create(workspaceCwd, agentDir, {
      projectTrusted: isProjectTrusted(workspaceCwd),
    })
  }
  const skills = new SkillsService({
    path: resolve(agentDir, 'pisper-skills.json'),
    agentDir,
    cwd,
    getSettingsManager,
    extensionFactories,
  })
  const extensions = new ExtensionsService({
    agentDir,
    cwd,
    getSettingsManager,
    createResourceLoader: (targetCwd) => skills.createResourceLoader(targetCwd),
  })
  return { skills, extensions }
}
