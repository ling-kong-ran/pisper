import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { migrateKimiCodeProvider } from '../services/provider-migrations.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

test('Kimi Code migration moves compatible credentials and default model', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-kimi-migration-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const paths = {
    authPath: join(directory, 'auth.json'),
    modelsPath: join(directory, 'models.json'),
    settingsPath: join(directory, 'settings.json'),
    appConfigPath: join(directory, 'pisper.json'),
  }
  await writeJsonAtomic(paths.authPath, {
    'moonshotai-cn': { type: 'api_key', key: 'sk-kimi-example' },
  })
  await writeJsonAtomic(paths.modelsPath, {
    providers: { 'moonshotai-cn': { baseUrl: 'https://api.moonshot.cn/v1' } },
  })
  await writeJsonAtomic(paths.settingsPath, {
    defaultProvider: 'moonshotai-cn',
    defaultModel: 'kimi-k3',
  })
  await writeJsonAtomic(paths.appConfigPath, { disabledProviders: ['moonshotai-cn'] })

  assert.equal(await migrateKimiCodeProvider(paths), true)
  const auth = await readJson(paths.authPath, {})
  const models = await readJson(paths.modelsPath, {})
  const settings = await readJson(paths.settingsPath, {})
  const appConfig = await readJson(paths.appConfigPath, {})
  assert.equal(auth['kimi-coding'].key, 'sk-kimi-example')
  assert.equal(auth['moonshotai-cn'], undefined)
  assert.equal(models.providers['moonshotai-cn'], undefined)
  assert.equal(settings.defaultProvider, 'kimi-coding')
  assert.equal(settings.defaultModel, 'k3')
  assert.deepEqual(appConfig.disabledProviders, ['kimi-coding'])
})
