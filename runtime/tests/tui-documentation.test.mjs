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

test('npm, Provider setup, and optional Web onboarding stay documented', async () => {
  const [cliSource, chineseReadme, englishReadme, chineseTuiGuide, englishTuiGuide, projectPage] =
    await Promise.all([
      readFile('src-tui/src/main.rs', 'utf8'),
      readFile('README.md', 'utf8'),
      readFile('README.en.md', 'utf8'),
      readFile('src-tui/README.md', 'utf8'),
      readFile('src-tui/README.en.md', 'utf8'),
      readFile('docs/index.html', 'utf8'),
    ])

  for (const document of [
    chineseReadme,
    englishReadme,
    chineseTuiGuide,
    englishTuiGuide,
    projectPage,
  ]) {
    assert.match(document, /npm install -g pisper/)
    assert.match(document, /pisper web/)
    assert.match(document, /\/apikey/)
  }
  for (const guide of [chineseTuiGuide, englishTuiGuide]) {
    assert.match(guide, /pisper update web/)
    assert.match(guide, /\| `\/web`/)
  }
  assert.match(cliSource, /pisper help \[COMMAND\]/)
  assert.match(cliSource, /pisper update web/)
  assert.match(cliSource, /Use `\/apikey`/)
})

test('desktop CLI management lives under App updates', async () => {
  const [configPage, updateSettings, chineseReadme, englishReadme] = await Promise.all([
    readFile('src/features/config/ConfigPage.tsx', 'utf8'),
    readFile('src/features/config/UpdateSettings.tsx', 'utf8'),
    readFile('README.md', 'utf8'),
    readFile('README.en.md', 'utf8'),
  ])

  assert.doesNotMatch(configPage, /section === 'terminal'/)
  assert.match(updateSettings, /<CliSettings notify=\{notify\}/)
  assert.match(chineseReadme, /设置 → 应用更新/)
  assert.match(englishReadme, /Settings → App updates/)
})
