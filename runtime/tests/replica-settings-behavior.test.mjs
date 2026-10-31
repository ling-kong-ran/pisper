import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { ensureCustomUiMessages } from '../../src/app/i18n.ts'
import { searchConfig } from '../../src/features/config/config-search.ts'

test('search targets request expansion of the group that owns the control', () => {
  const policy = searchConfig('运行策略', 'zh-CN').find(
    ({ entry }) => entry.id === 'models-runtime-policy',
  )?.entry
  // A6 评审返修：思考强度与可用工具是页标题下的默认值行，常显，不再需要展开“更多设置”。
  assert.equal(policy?.card, 'models-current-model')
  assert.equal(policy?.targetId, 'models-runtime-policy')
  assert.equal(policy?.expandGroup, undefined)
  const visual = searchConfig('visual', 'en-US').find(
    ({ entry }) => entry.id === 'models-visual',
  )?.entry
  assert.equal(visual?.expandGroup, 'models-advanced')
  const font = searchConfig('font', 'en-US').find(
    ({ entry }) => entry.id === 'interface-style',
  )?.entry
  assert.equal(font?.expandGroup, 'interface-advanced')
})

test('search omits controls and sections unavailable on the current runtime', () => {
  const options = {
    capabilities: {
      features: { desktopPet: false, remoteAccess: false, visualGeneration: false },
    },
    mobileApp: true,
  }
  assert.deepEqual(searchConfig('pet', 'en-US', options), [])
  assert.deepEqual(searchConfig('visual', 'en-US', options), [])
  assert.ok(
    searchConfig('server', 'en-US', options).some(({ entry }) => entry.section === 'mobile-server'),
  )
  assert.ok(
    !searchConfig('remote', 'en-US', options).some(
      ({ entry }) => entry.section === 'remote-access',
    ),
  )
  assert.ok(
    !searchConfig('server', 'en-US', { mobileApp: false }).some(
      ({ entry }) => entry.section === 'mobile-server',
    ),
  )
})

// —— 第 4 期设置重组（规格 2.6）——

test('the retired standalone widgets tab is still found by search and leads to the components page', async () => {
  // 面包屑文案在随设置路由加载的 custom-ui 命名空间里，先装上再取译文。
  await ensureCustomUiMessages()
  for (const [query, language, title, breadcrumb] of [
    ['独立组件', 'zh-CN', '组件工作室', '组件已移到 功能 › 组件工作室 →'],
    [
      'standalone widgets',
      'en-US',
      'Component studio',
      'Components moved to Features › Component studio →',
    ],
  ]) {
    const hits = searchConfig(query, language)
    assert.deepEqual(
      hits.map(({ entry }) => entry.id),
      ['interface-custom-ui'],
      query,
    )
    assert.equal(hits[0].title, title)
    assert.equal(hits[0].sectionTitle, breadcrumb)
  }
  // 选中后不登记卡片高亮（设置里已没有这张卡），交给应用壳的 ?view=widgets 去组件页。
  const box = await readFile('src/features/config/ConfigSearch.tsx', 'utf8')
  const select = box.slice(box.indexOf('const select = (match: ConfigSearchMatch) => {'))
  const widgets = select.search(
    /if \(match\.entry\.card === 'interface-custom-ui'\) \{\s*onSelect\(match\.entry\.section, 'widgets'\)\s*return\s*\}/,
  )
  assert.ok(widgets > 0 && widgets < select.indexOf('requestConfigCardHighlight('))
  // 其余条目不再带 view 参数（界面设置已经没有标签页可切）。
  assert.equal(box.match(/onSelect\(match\.entry\.section, /g)?.length, 1)
})

test('the default accent preset is listed first and named Graphite in both languages', async () => {
  const [zh, en, settings] = await Promise.all([
    readFile('src/locales/zh-CN/config.json', 'utf8').then(JSON.parse),
    readFile('src/locales/en-US/config.json', 'utf8').then(JSON.parse),
    readFile('src/features/config/InterfaceSettings.tsx', 'utf8'),
  ])
  assert.equal(zh['interfaceSettings.accentNeutral'], '石墨')
  assert.equal(en['interfaceSettings.accentNeutral'], 'Graphite')
  assert.match(
    settings,
    /const accentOptions: Array<\{ value: AccentPreset; label: string \}> = \[\s*\{ value: 'neutral', label: t\('config:interfaceSettings\.accentNeutral'\) \}/,
  )
})
