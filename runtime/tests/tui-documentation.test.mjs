import assert from 'node:assert/strict'
import { access, readFile } from 'node:fs/promises'
import test from 'node:test'

const commandPattern = /command\(\s*"([^"]+)"/g

test('TUI guides cover every built-in Slash command and the current screenshots', async () => {
  const [source, chinese, english] = await Promise.all([
    readFile('src-tui/src/app.rs', 'utf8'),
    readFile('src-tui/README.md', 'utf8'),
    readFile('src-tui/README.en.md', 'utf8'),
    access('docs/shots/cli.png'),
    access('docs/shots/cli-chat.png'),
  ])
  const commands = [...source.matchAll(commandPattern)].map((match) => match[1])

  assert.ok(commands.length > 0)
  for (const command of new Set(commands)) {
    assert.ok(chinese.includes(`| \`${command}`), `Chinese guide is missing ${command}`)
    assert.ok(english.includes(`| \`${command}`), `English guide is missing ${command}`)
  }
  assert.match(chinese, /\[项目主页\]\(https:\/\/ling-kong-ran\.github\.io\/pisper\/\)/)
  assert.match(english, /\[Project home\]\(https:\/\/ling-kong-ran\.github\.io\/pisper\/\)/)
  for (const guide of [chinese, english]) {
    assert.match(guide, /https:\/\/ling-kong-ran\.github\.io\/pisper\/shots\/cli\.png/)
    assert.match(guide, /https:\/\/ling-kong-ran\.github\.io\/pisper\/shots\/cli-chat\.png/)
    assert.match(guide, /https:\/\/github\.com\/ling-kong-ran\/pisper/)
  }
})

test('npm, Provider setup, and optional Web onboarding stay documented', async () => {
  const [
    cliSource,
    npmUpdateSource,
    chineseReadme,
    englishReadme,
    chineseTuiGuide,
    englishTuiGuide,
    projectPage,
  ] = await Promise.all([
    readFile('src-tui/src/main.rs', 'utf8'),
    readFile('packages/pisper/lib/npm-update.mjs', 'utf8'),
    readFile('README.md', 'utf8'),
    readFile('README.en.md', 'utf8'),
    readFile('src-tui/README.md', 'utf8'),
    readFile('src-tui/README.en.md', 'utf8'),
    readFile('docs/index.html', 'utf8'),
  ])

  for (const guide of [chineseTuiGuide, englishTuiGuide]) {
    assert.match(guide, /npm install -g pisper/)
    assert.match(guide, /pisper web/)
    assert.match(guide, /\/provider/)
  }
  for (const readme of [chineseReadme, englishReadme]) {
    assert.match(readme, /https:\/\/ling-kong-ran\.github\.io\/pisper\//)
    assert.doesNotMatch(readme, /npm install -g pisper|pisper web|\/provider/)
  }
  assert.match(projectPage, /npm i -g pisper/)
  assert.doesNotMatch(projectPage, /npm install -g pisper/)
  assert.match(projectPage, /data-copy-install/)
  assert.match(projectPage, /pisper web/)
  assert.match(projectPage, /\/provider/)
  for (const guide of [chineseTuiGuide, englishTuiGuide]) {
    assert.match(guide, /pisper update --check/)
    assert.match(guide, /\| `\/web`/)
    assert.doesNotMatch(guide, /pisper update (?:tui|runtime|web|all)/)
  }
  assert.match(cliSource, /pisper help \[COMMAND\]/)
  assert.doesNotMatch(cliSource, /  pisper update/)
  assert.doesNotMatch(cliSource, /  update\s+Update/)
  assert.match(npmUpdateSource, /pisper update \[--check\]/)
  assert.doesNotMatch(npmUpdateSource, /pisper update \[COMPONENT\]/)
  assert.match(cliSource, /Use `\/provider`/)
})

test('desktop CLI management lives under App updates', async () => {
  const [configPage, updateSettings, chineseGuide, englishGuide] = await Promise.all([
    readFile('src/features/config/ConfigPage.tsx', 'utf8'),
    readFile('src/features/config/UpdateSettings.tsx', 'utf8'),
    readFile('src-tui/README.md', 'utf8'),
    readFile('src-tui/README.en.md', 'utf8'),
  ])

  assert.doesNotMatch(configPage, /section === 'terminal'/)
  assert.match(updateSettings, /<CliSettings notify=\{notify\}/)
  assert.match(chineseGuide, /设置 → 应用更新/)
  assert.match(englishGuide, /Settings → App updates/)
})
