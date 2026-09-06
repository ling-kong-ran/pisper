import assert from 'node:assert/strict'
import { access, readFile } from 'node:fs/promises'
import test from 'node:test'
import { searchConfig } from '../../src/features/config/config-search.ts'

test('model settings have no speech hotword controls or component', async () => {
  const models = await readFile('src/features/config/ModelsSettings.tsx', 'utf8')
  assert.doesNotMatch(models, /SpeechTermsSettings|models-speech|projectTermsEnabled|builtinTerms/)
  await assert.rejects(access('src/features/config/SpeechTermsSettings.tsx'), { code: 'ENOENT' })
})

for (const language of ['zh-CN', 'en-US']) {
  test(`hotword settings are absent from search and translations (${language})`, async () => {
    for (const query of ['hotwords', 'speech hotwords', '热词', '项目词', '内置词']) {
      assert.deepEqual(searchConfig(query, language), [], query)
    }
    const messages = JSON.parse(await readFile(`src/locales/${language}/config.json`, 'utf8'))
    assert.equal(
      Object.keys(messages).some((key) => key.startsWith('speechTermsSettings.')),
      false,
    )
    assert.ok(searchConfig('voice', language).some((match) => match.entry.section === 'shortcuts'))
  })
}
