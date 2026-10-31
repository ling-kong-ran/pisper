import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { HOME_ENTRY_STATE, isHomeEntry, pageFromPath } from '../../src/app/routes.ts'
import { settingsOwnsRoute } from '../../src/app/settings-navigation.ts'
import { shellBackPath } from '../../src/app/shell-tabs.ts'

test('native back delegates only to the trusted app and retains the platform fallback', async () => {
  const source = await readFile('src-tauri/mobile/android/MainActivity.kt', 'utf8')
  const handler = source.slice(
    source.indexOf('override fun handleOnBackPressed()'),
    source.indexOf('// Android 15+'),
  )
  assert.match(handler, /!isTrustedProxyOrigin\(origin\)/)
  assert.ok(
    handler.indexOf('!isTrustedProxyOrigin(origin)') < handler.indexOf('evaluateJavascript'),
  )
  assert.match(handler, /__pisperRequestBack/)
  assert.match(handler, /consumed != "true"\) fallback\(\)/)
  assert.match(handler, /onBackPressedDispatcher\.onBackPressed\(\)/)
  assert.match(handler, /isEnabled = false[\s\S]*finally \{ isEnabled = true \}/)
})

test('the app hands unconsumed native back to the route hierarchy only in the mobile layout', async () => {
  const app = await readFile('src/App.tsx', 'utf8')
  // 原生返回未被弹层消费时交给路由层：只在移动布局（含手机 App 平板）挂上；桌面不传，直接交还平台。
  assert.match(app, /<BackNavigation onRouteBack=\{mobileLayout \? routeBack : undefined\} \/>/)
  const routeBack = app.slice(app.indexOf('const routeBack = useCallback'))
  const body = routeBack.slice(0, routeBack.indexOf('}, ['))
  // 上一级只由 shellBackPath 决定（第 4 期起会话主页是根，见 session-home.test）：手机外壳、
  // 主页落地标记与“当前条目之前还有应用内条目”（React Router 的 history.state.idx > 0）三项输入。
  assert.match(body, /const index: unknown = window\.history\.state\?\.idx/)
  assert.match(
    body,
    /shellBackPath\(page, location\.pathname, settingsOwned, \{\s*phone: phoneShell,\s*homeEntry: isHomeEntry\(location\.state\),\s*canGoBack: typeof index === 'number' && index > 0,\s*\}\)/,
  )
  // -1 走历史回到来处，路径则前往上一级；null（根）返回 false，由 BackNavigation 交还平台。
  assert.match(
    body,
    /if \(parent === -1\) routerNavigate\(-1\)\s+else if \(parent\) routerNavigate\(parent\)\s+return parent !== null/,
  )
  assert.doesNotMatch(body, /routerNavigate\(PAGE_PATHS\.board\)/)
})

test('repeated native back from any page climbs to the session home and then yields to the platform', () => {
  // 不依赖历史（冷启动、通知或深链直接打开）时逐级返回：每一步都换到上一级页面，最多三步到根，
  // 根（主页或从主页落地的会话）交还平台；不会在两个页面之间来回跳。
  const climb = (path, { state, phone = false } = {}) => {
    const visited = [path]
    let current = path
    let currentState = state
    for (let step = 0; step < 6; step += 1) {
      const page = pageFromPath(current)
      const section = current.split('/')[2] || 'models'
      const parent = shellBackPath(page, current, settingsOwnsRoute(page, section), {
        phone,
        homeEntry: isHomeEntry(currentState),
        canGoBack: false,
      })
      if (parent === null) return visited
      assert.equal(typeof parent, 'string', `${current} must name its parent without history`)
      assert.ok(!visited.includes(parent), `${visited.join(' → ')} → ${parent} loops`)
      visited.push(parent)
      current = parent
      currentState = undefined
    }
    assert.fail(`${path} never reaches the platform`)
  }
  assert.deepEqual(climb('/'), ['/'])
  assert.deepEqual(climb('/board/abc'), ['/board/abc'])
  assert.deepEqual(climb('/board/abc', { state: HOME_ENTRY_STATE }), ['/board/abc'])
  assert.deepEqual(climb('/board'), ['/board', '/'])
  assert.deepEqual(climb('/config/interface'), ['/config/interface', '/'])
  assert.deepEqual(climb('/assets'), ['/assets', '/more', '/'])
  // 双归属退役（规格 2.6）：插件只在功能里，旧版写进历史条目的进入来源不再影响上一级；
  // /config 下的远程访问 / 手机服务器 / 桌面宠物是功能页，同样先回功能。
  for (const state of [undefined, { navOrigin: 'activity' }])
    assert.deepEqual(climb('/plugins', { state }), ['/plugins', '/more', '/'])
  assert.deepEqual(climb('/config/mobile-server'), ['/config/mobile-server', '/more', '/'])
  assert.deepEqual(climb('/components/game-assets'), [
    '/components/game-assets',
    '/components',
    '/',
  ])
  // 手机没有“组件”标签：组件详情 → 列表 → 更多 → 主页。
  assert.deepEqual(climb('/components/game-assets', { phone: true }), [
    '/components/game-assets',
    '/components',
    '/more',
    '/',
  ])
})
