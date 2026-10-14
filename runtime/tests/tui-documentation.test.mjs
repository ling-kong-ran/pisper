import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

const commandPattern = /command\(\s*"([^"]+)"/g

test('TUI command references cover every built-in Slash command without product-guide content', async () => {
  const [source, chinese, english] = await Promise.all([
    readFile('src-tui/src/app.rs', 'utf8'),
    readFile('src-tui/README.md', 'utf8'),
    readFile('src-tui/README.en.md', 'utf8'),
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
    assert.match(guide, /https:\/\/github\.com\/ling-kong-ran\/pisper/)
    assert.match(guide, /\| `pisper resume`/)
    assert.match(guide, /\| `pisper doctor`/)
    assert.match(guide, /\| `pisper web`/)
    assert.doesNotMatch(guide, /!\[/)
    assert.doesNotMatch(guide, /npm install -g pisper/)
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
    assert.doesNotMatch(guide, /npm install -g pisper/)
    assert.match(guide, /pisper web/)
    assert.match(guide, /\/provider/)
  }
  for (const readme of [chineseReadme, englishReadme]) {
    assert.match(readme, /https:\/\/ling-kong-ran\.github\.io\/pisper\//)
    // 根 README 改为自带三分钟上手(转化导向):必须包含 npm 入口,
    // 同时仍链接 TUI 命令参考作为 CLI 与 Slash command 的权威文档。
    assert.match(readme, /npm (?:i|install) -g pisper/)
    assert.match(readme, /src-tui\/README/)
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
  const [configPage, updateSettings] = await Promise.all([
    readFile('src/features/config/ConfigPage.tsx', 'utf8'),
    readFile('src/features/config/UpdateSettings.tsx', 'utf8'),
  ])

  assert.doesNotMatch(configPage, /section === 'terminal'/)
  assert.match(updateSettings, /<CliSettings notify=\{notify\}/)
})
