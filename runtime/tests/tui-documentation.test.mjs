import assert from 'node:assert/strict'
import { access, readFile } from 'node:fs/promises'
import test from 'node:test'

const commandPattern = /command\(\s*"([^"]+)"/g

test('TUI guides cover every built-in Slash command and the current screenshot', async () => {
  const [source, chinese, english] = await Promise.all([
    readFile('src-tui/src/app.rs', 'utf8'),
    readFile('src-tui/README.md', 'utf8'),
    readFile('src-tui/README.en.md', 'utf8'),
    access('docs/shots/cli.png'),
  ])
  const commands = [...source.matchAll(commandPattern)].map((match) => match[1])

  assert.ok(commands.length > 0)
  for (const command of new Set(commands)) {
    assert.ok(chinese.includes(`| \`${command}`), `Chinese guide is missing ${command}`)
    assert.ok(english.includes(`| \`${command}`), `English guide is missing ${command}`)
  }
  assert.match(chinese, /\.\.\/docs\/shots\/cli\.png/)
  assert.match(english, /\.\.\/docs\/shots\/cli\.png/)
})

test('desktop CLI management has a dedicated Terminal settings section', async () => {
  const [configPage, languageSettings, chineseReadme, englishReadme] = await Promise.all([
    readFile('src/features/config/ConfigPage.tsx', 'utf8'),
    readFile('src/features/config/LanguageSettings.tsx', 'utf8'),
    readFile('README.md', 'utf8'),
    readFile('README.en.md', 'utf8'),
  ])

  assert.match(configPage, /section === 'terminal'/)
  assert.match(configPage, /<CliSettings notify=\{notify\}/)
  assert.doesNotMatch(languageSettings, /CliSettings/)
  assert.match(chineseReadme, /设置 → 终端/)
  assert.match(englishReadme, /Settings → Terminal/)
})
