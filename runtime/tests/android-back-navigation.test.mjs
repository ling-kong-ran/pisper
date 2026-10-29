import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
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
  // 原生返回未被弹层消费时：会话页回看板，从“更多”进入的功能页回更多，其余交还平台。
  assert.match(app, /<BackNavigation onRouteBack=\{mobileLayout \? routeBack : undefined\} \/>/)
  const routeBack = app.slice(app.indexOf('const routeBack = useCallback'))
  assert.match(
    routeBack.slice(0, 260),
    /if \(page === 'session'\) backToBoard\(\)\s+else if \(moreOwned\) backToMore\(\)\s+else return false\s+return true/,
  )
  assert.match(app, /const moreOwned = shellTab === 'more' && page !== 'more'/)
  assert.match(app, /const backToBoard = useCallback\(\(\) => routerNavigate\(PAGE_PATHS\.board\)/)
  assert.match(app, /const backToMore = useCallback\(\(\) => routerNavigate\(PAGE_PATHS\.more\)/)
})
