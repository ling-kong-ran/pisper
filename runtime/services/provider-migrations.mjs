// Provider 配置迁移：把历史版本的 Provider 配置（如 moonshotai-cn → kimi-coding）
// 迁移到当前命名，避免用户旧配置失效。
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

const KIMI_CODE_MODEL_MAP = {
  'kimi-k3': 'k3',
  'kimi-k2.7-code': 'kimi-for-coding',
  'kimi-k2.7-code-highspeed': 'kimi-for-coding-highspeed',
}

export async function migrateKimiCodeProvider({
  authPath,
  modelsPath,
  settingsPath,
  appConfigPath,
}) {
  let changed = false
  const auth = await readJson(authPath, {})
  const oldCredential = auth['moonshotai-cn']
  if (String(oldCredential?.key || '').startsWith('sk-kimi-')) {
    auth['kimi-coding'] ||= oldCredential
    delete auth['moonshotai-cn']
    await writeJsonAtomic(authPath, auth)
    changed = true
  }

  const models = await readJson(modelsPath, { providers: {} })
  if (models.providers?.['moonshotai-cn']) {
    delete models.providers['moonshotai-cn']
    await writeJsonAtomic(modelsPath, models)
    changed = true
  }

  const settings = await readJson(settingsPath, {})
  if (settings.defaultProvider === 'moonshotai-cn') {
    settings.defaultProvider = 'kimi-coding'
    settings.defaultModel = KIMI_CODE_MODEL_MAP[settings.defaultModel] || 'kimi-for-coding'
    await writeJsonAtomic(settingsPath, settings)
    changed = true
  }

  const appConfig = await readJson(appConfigPath, {})
  if (
    Array.isArray(appConfig.disabledProviders) &&
    appConfig.disabledProviders.includes('moonshotai-cn')
  ) {
    appConfig.disabledProviders = [
      ...new Set(
        appConfig.disabledProviders.map((provider) =>
          provider === 'moonshotai-cn' ? 'kimi-coding' : provider,
        ),
      ),
    ]
    await writeJsonAtomic(appConfigPath, appConfig)
    changed = true
  }
  return changed
}
