import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { getActivityNavigation } from '../../src/app/activity-navigation.ts'
import {
  MORE_PAGES,
  countMoreTiles,
  filterMoreNavigation,
  getMoreNavigation,
} from '../../src/app/more-navigation.ts'
import { SHELL_TAB_PAGES } from '../../src/app/shell-tabs.ts'
import { parseDisconnectedChannelCount } from '../../src/features/channels/channel-status.ts'
import { requestCommandPalette, takeCommandPaletteQuery } from '../../src/features/chat/events.ts'
import { parseOnlineMcpCount } from '../../src/features/mcp/mcp-status.ts'
import { parseEnabledScheduleCount } from '../../src/features/schedules/schedule-status.ts'
import { parseRunningWorkflowCount } from '../../src/features/workflows/workflow-status.ts'
import { LEGACY_RUNTIME_CAPABILITIES } from '../../src/types/runtime-capabilities.ts'

// “更多”启动台取代旧功能栏分组（activity-navigation）在外壳里的角色（规格 2.5）。
const t = (key) => key

function capabilities(features = {}) {
  return {
    ...LEGACY_RUNTIME_CAPABILITIES,
    features: { ...LEGACY_RUNTIME_CAPABILITIES.features, ...features },
  }
}

const tiles = (groups) => groups.flatMap((group) => group.tiles)
const pageTiles = (groups) =>
  tiles(groups)
    .filter((tile) => tile.destination.type === 'page')
    .map((tile) => tile.destination.id)

test('more launcher groups follow the spec order and hide runtime-trimmed pages', () => {
  const groups = getMoreNavigation(t)
  assert.deepEqual(
    groups.map((group) => group.id),
    ['sessions', 'automation', 'extensions', 'devices', 'components'],
  )
  assert.deepEqual(
    groups.map((group) => [group.id, group.tiles.map((tile) => tile.key)]),
    [
      ['sessions', ['page:chatHistory', 'page:assets', 'page:memory']],
      ['automation', ['page:workflows', 'page:schedules', 'page:channels']],
      ['extensions', ['page:plugins', 'page:skills', 'page:mcp']],
      ['devices', ['config:remote-access', 'config:desktop-pet']],
      ['components', ['page:components']],
    ],
  )
  const trimmed = getMoreNavigation(t, {
    capabilities: capabilities({ workflows: false, schedules: false, channels: false }),
  })
  assert.equal(
    trimmed.some((group) => group.id === 'automation'),
    false,
  )
  // 桌面：远程访问；Runtime 裁掉远程访问与桌面宠物时整组隐藏。
  const desktopTrimmed = getMoreNavigation(t, {
    capabilities: capabilities({ remoteAccess: false, desktopPet: false }),
  })
  assert.equal(
    desktopTrimmed.some((group) => group.id === 'devices'),
    false,
  )
})

test('capability trimming hides exactly the unavailable pages and keeps the rest of each group', () => {
  const groups = getMoreNavigation(t, {
    capabilities: capabilities({ assets: false, memory: false, skills: false, mcp: false }),
  })
  // 历史会话与插件不受能力裁剪，所在分组保留；被裁掉的页面不以禁用磁贴出现。
  assert.deepEqual(
    groups.find((group) => group.id === 'sessions').tiles.map((tile) => tile.key),
    ['page:chatHistory'],
  )
  assert.deepEqual(
    groups.find((group) => group.id === 'extensions').tiles.map((tile) => tile.key),
    ['page:plugins'],
  )
  for (const page of ['assets', 'memory', 'skills', 'mcp'])
    assert.equal(pageTiles(groups).includes(page), false, page)
  assert.equal(
    tiles(groups).some((tile) => tile.disabledReason),
    false,
  )
})

test('every destination of the retired activity rail stays reachable from the new shell', () => {
  // 旧功能栏能到的页面，新外壳里要么是一级标签，要么是“更多”里的磁贴；能力裁剪结论保持一致。
  for (const features of [
    {},
    { skills: false, workflows: false, assets: false, channels: false },
  ]) {
    const caps = capabilities(features)
    const reachable = new Set([
      ...Object.values(SHELL_TAB_PAGES),
      ...pageTiles(getMoreNavigation(t, { capabilities: caps })),
    ])
    const activity = getActivityNavigation(t, caps).flatMap((group) =>
      group.items.map((item) => item.id),
    )
    for (const page of activity) {
      // 旧的“聊天”入口由看板标签承担，会话在看板上打开。
      const target = page === 'chat' ? 'board' : page
      assert.ok(reachable.has(target), `${page} is no longer reachable`)
    }
    for (const page of pageTiles(getMoreNavigation(t, { capabilities: caps })))
      assert.ok(
        page === 'components' || activity.includes(page),
        `${page} is reachable from More but trimmed by capabilities`,
      )
  }
})

test('tiles are unique, described and own the More tab when opened', () => {
  for (const options of [{}, { mobileApp: true }]) {
    const groups = getMoreNavigation(t, {
      ...options,
      components: [{ id: 'game-assets', name: '游戏素材工作台' }],
    })
    const keys = tiles(groups).map((tile) => tile.key)
    assert.equal(keys.length, new Set(keys).size)
    for (const tile of tiles(groups)) {
      assert.ok(tile.label, tile.key)
      assert.ok(tile.description, tile.key)
      assert.ok(tile.icon, tile.key)
    }
    // 从“更多”打开的功能页点亮“更多”标签（组件页除外，它本身是一级标签）。
    for (const page of pageTiles(groups))
      assert.ok(page === 'components' || MORE_PAGES.has(page), page)
  }
})

test('built-in tile descriptions fit the 360px phone tile without mid-phrase breaks', async () => {
  // 手机三列磁贴在 360px 宽时说明只有约 78px：12px 中文一行最多 6 个字，再长就会在词中间折行
  // （如“生成的图 / 片与文件”）；英文最多两行、只在空格处折行，所以限总长与最长单词。真实折行由更多页截图核对。
  const [zh, en] = await Promise.all(
    ['zh-CN', 'en-US'].map(async (language) =>
      JSON.parse(await readFile(`src/locales/${language}/navigation.json`, 'utf8')),
    ),
  )
  const keys = new Set()
  for (const options of [{}, { mobileApp: true, componentsTile: true }])
    for (const tile of tiles(
      getMoreNavigation(t, { ...options, components: [{ id: 'game-assets', name: 'x' }] }),
    ))
      for (const value of [tile.description, tile.disabledReason])
        if (value) keys.add(value.replace(/^navigation:/, ''))
  assert.ok(keys.has('more.componentsHint') && keys.has('more.desktopOnly'))
  for (const key of keys) {
    assert.ok(zh[key] && [...zh[key]].length <= 6, `zh-CN ${key}: ${zh[key]}`)
    assert.ok(en[key] && en[key].length <= 25, `en-US ${key}: ${en[key]}`)
    assert.ok(Math.max(...en[key].split(' ').map((word) => word.length)) <= 11, en[key])
  }
})

test('phones swap in the server tile and show desktop pet disabled with a reason', () => {
  const devices = getMoreNavigation(t, {
    mobileApp: true,
    capabilities: capabilities({ desktopPet: false, remoteAccess: false }),
  }).find((group) => group.id === 'devices')
  assert.deepEqual(
    devices.tiles.map((tile) => [tile.key, tile.disabledReason || '']),
    [
      ['config:mobile-server', ''],
      ['config:desktop-pet', 'navigation:more.desktopOnly'],
    ],
  )
  // 桌面上桌面宠物是可用磁贴，不带禁用原因。
  const desktop = getMoreNavigation(t).find((group) => group.id === 'devices')
  assert.equal(
    desktop.tiles.some((tile) => tile.disabledReason),
    false,
  )
})

test('my components list installed tools and always end with the add tile', () => {
  const groups = getMoreNavigation(t, {
    components: [{ id: 'game-assets', name: '游戏素材工作台', description: '' }],
  })
  const components = groups.find((group) => group.id === 'components')
  assert.deepEqual(
    components.tiles.map((tile) => tile.destination),
    [
      { type: 'component', id: 'game-assets' },
      { type: 'page', id: 'components' },
    ],
  )
  assert.equal(components.tiles[0].description, 'navigation:more.componentHint')
  assert.deepEqual(countMoreTiles(groups), { features: 11, components: 1 })
  assert.deepEqual(countMoreTiles(getMoreNavigation(t)), { features: 11, components: 0 })
})

test('more search filters tiles by name, description or group and drops empty groups', () => {
  const groups = getMoreNavigation((key) =>
    key === 'navigation:navigation.mcp'
      ? 'MCP'
      : key === 'navigation:more.mcpHint'
        ? '连接外部工具'
        : key,
  )
  assert.deepEqual(
    filterMoreNavigation(groups, 'mcp').flatMap((group) => group.tiles.map((tile) => tile.key)),
    ['page:mcp'],
  )
  assert.deepEqual(
    filterMoreNavigation(groups, '外部').flatMap((group) => group.tiles.map((tile) => tile.key)),
    ['page:mcp'],
  )
  // 分组名命中时保留整组磁贴。
  assert.deepEqual(
    filterMoreNavigation(groups, 'navigation:more.groupAutomation').flatMap((group) =>
      group.tiles.map((tile) => tile.key),
    ),
    ['page:workflows', 'page:schedules', 'page:channels'],
  )
  assert.equal(filterMoreNavigation(groups, '   ').length, groups.length)
  assert.deepEqual(filterMoreNavigation(groups, 'no-such-feature'), [])
})

test('more tile badges read only honest counts and reject malformed payloads', () => {
  // 角标数据来自 Runtime 的只读接口，按不可信数据校验：非法结构抛错（角标不显示），不猜数字。
  assert.equal(parseRunningWorkflowCount({ limits: { running: 2, maxConcurrent: 4 } }), 2)
  assert.throws(() => parseRunningWorkflowCount({ limits: { running: '2' } }))
  assert.throws(() => parseRunningWorkflowCount({}))
  assert.equal(
    parseEnabledScheduleCount({
      tasks: [{ enabled: true }, { enabled: false }, null, { enabled: 1 }],
    }),
    1,
  )
  assert.throws(() => parseEnabledScheduleCount({ tasks: {} }))
  // 渠道：启用但没连上的才算断开；连接中 / 重连中、已停用、未配置都不算。
  assert.equal(
    parseDisconnectedChannelCount({
      connections: {
        feishu: { enabled: true, status: 'failed' },
        weixin: { enabled: true, status: 'idle' },
        qq: { enabled: true, status: 'reconnecting' },
        telegram: { enabled: false, status: 'failed' },
        other: null,
        connected: { enabled: true, status: 'connected' },
      },
    }),
    2,
  )
  assert.throws(() => parseDisconnectedChannelCount({ connections: [] }))
  assert.equal(parseOnlineMcpCount({ metrics: { onlineServices: 3 } }), 3)
  assert.throws(() => parseOnlineMcpCount({ metrics: { onlineServices: -1 } }))
})

test('the more search hands its keyword to the command palette exactly once', () => {
  const events = []
  globalThis.window ??= new EventTarget()
  const listener = (event) => events.push(event.type)
  window.addEventListener('pisper:command-palette-requested', listener)
  try {
    requestCommandPalette('主题')
    assert.equal(takeCommandPaletteQuery(), '主题')
    assert.equal(takeCommandPaletteQuery(), '')
    // 直接作按钮 onClick 时传进来的是点击事件：不当作关键词。
    requestCommandPalette({ type: 'click' })
    assert.equal(takeCommandPaletteQuery(), '')
    assert.equal(events.length, 2)
  } finally {
    window.removeEventListener('pisper:command-palette-requested', listener)
  }
})
