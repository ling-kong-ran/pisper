// 右侧浏览器：地址栏的网址 / 搜索判断、落盘前的地址清洗、每个会话页签的序列化与恢复、浏览历史。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { BROWSER_PREFERENCE_KEYS } from '../../shared/browser-preferences.mjs'
import { browserSearchUrl, resolveBrowserAddress } from '../../src/features/chat/browser-address.ts'
import {
  browserTabsSnapshot,
  isSecretQueryParam,
  parseBrowserTabs,
  PERSISTED_BROWSER_SESSIONS,
  PERSISTED_BROWSER_TABS,
  PERSISTED_BROWSER_TABS_MAX_BYTES,
  sanitizeBrowserTitle,
  sanitizeBrowserUrl,
  serializeBrowserTabs,
} from '../../src/features/chat/browser-persistence.ts'
import {
  BROWSER_HISTORY_MAX_BYTES,
  groupBrowserHistory,
  MAX_BROWSER_HISTORY,
  parseBrowserHistory,
  recordBrowserVisit,
  serializeBrowserHistory,
  updateBrowserHistoryTitle,
} from '../../src/features/chat/browser-history.ts'
import {
  createContextPages,
  restoreBrowserPages,
  updateContextPages as update,
} from '../../src/features/chat/session-context-pages.ts'
import { SESSION_BROWSER_TABS_KEY } from '../../src/features/chat/session-context-pages-store.ts'
import { BROWSER_HISTORY_KEY } from '../../src/features/chat/browser-history-store.ts'

const base = 'http://127.0.0.1:5173/session/abc'
const app = 'http://127.0.0.1:5173'

test('the address bar opens addresses and searches everything else on Bing', () => {
  const url = (input) => resolveBrowserAddress(input, base)
  // 网址：补 https；本机和 IP 用 http；保留端口和路径。
  assert.deepEqual(url('example.com'), { kind: 'url', url: 'https://example.com/' })
  assert.deepEqual(url('docs.example.co.uk/guide?q=1'), {
    kind: 'url',
    url: 'https://docs.example.co.uk/guide?q=1',
  })
  assert.deepEqual(url('https://example.com/a b'), {
    kind: 'url',
    url: 'https://example.com/a%20b',
  })
  assert.deepEqual(url('HTTP://Example.com'), { kind: 'url', url: 'http://example.com/' })
  assert.deepEqual(url('localhost:5173/app'), { kind: 'url', url: 'http://localhost:5173/app' })
  assert.deepEqual(url('localhost'), { kind: 'url', url: 'http://localhost/' })
  assert.deepEqual(url('api.localhost:8080'), { kind: 'url', url: 'http://api.localhost:8080/' })
  assert.deepEqual(url('192.168.1.20:3000'), { kind: 'url', url: 'http://192.168.1.20:3000/' })
  assert.deepEqual(url('8.8.8.8'), { kind: 'url', url: 'http://8.8.8.8/' })
  assert.deepEqual(url('[::1]:8080/x'), { kind: 'url', url: 'http://[::1]:8080/x' })
  assert.deepEqual(url('printer.local'), { kind: 'url', url: 'http://printer.local/' })
  assert.deepEqual(url('例子.中国'), { kind: 'url', url: 'https://xn--fsqu00a.xn--fiqs8s/' })
  assert.deepEqual(url('  github.com/anthropics  '), {
    kind: 'url',
    url: 'https://github.com/anthropics',
  })

  // 搜索：有空格、没有点、最后一段不像顶级域、邮箱、非 http 协议。
  for (const input of [
    'rust ownership',
    'what is example.com',
    'pisper',
    'v1.2',
    '3.14',
    'myserver:3000',
    'me@example.com',
    'javascript:alert(1)',
    'file:///etc/passwd',
    'about:blank',
    '1.2.3.999',
    'c++',
    '天气',
  ]) {
    assert.deepEqual(url(input), { kind: 'search', url: browserSearchUrl(input) }, input)
  }
  assert.equal(
    browserSearchUrl('a&b c/天气'),
    'https://www.bing.com/search?q=a%26b%20c%2F%E5%A4%A9%E6%B0%94',
  )
  // 明确写了 http(s) 却打不开的地址报无效，不悄悄改成搜索；空输入什么也不做。
  assert.equal(url('https://'), null)
  assert.equal(url('   '), null)
})

test('stored browser addresses drop credentials, fragments and app-owned token URLs', () => {
  assert.equal(
    sanitizeBrowserUrl('https://user:pass@example.com/path?q=rust&page=2#section'),
    'https://example.com/path?q=rust&page=2',
  )
  assert.equal(
    sanitizeBrowserUrl(
      'https://example.com/cb?code=abc&state=1&access_token=t&id_token=x&X-Amz-Signature=s&sig=1&api_key=k&apikey=k&JSESSIONID=s&auth=1&ticket=t&otp=1&password=p&passwd=p&key=k&author=me',
    ),
    'https://example.com/cb?state=1&author=me',
  )
  // 不改写没有凭据参数的查询串：%20 不会被改成 +。
  assert.equal(
    sanitizeBrowserUrl('https://www.bing.com/search?q=a%20b'),
    'https://www.bing.com/search?q=a%20b',
  )
  for (const name of ['token', 'Session', 'refresh_token', 'client-secret', 'otp', 'sig'])
    assert.equal(isSecretQueryParam(name), true, name)
  for (const name of ['q', 'page', 'author', 'keyword', 'zipcode', 'lang'])
    assert.equal(isSecretQueryParam(name), false, name)

  // 应用自己的地址：工作区 HTML 预览、桌面引导，以及与应用同源的其他页面。
  assert.equal(sanitizeBrowserUrl(`${app}/_pisper/html-preview/tok/index.html`, app), null)
  assert.equal(sanitizeBrowserUrl('http://127.0.0.1:61234/_pisper/html-preview/tok/a.html'), null)
  assert.equal(sanitizeBrowserUrl(`${app}/_pisper/desktop/bootstrap?token=t`), null)
  assert.equal(sanitizeBrowserUrl(`${app}/api/sessions`, app), null)
  // 别的本机开发服务照常保存。
  assert.equal(sanitizeBrowserUrl('http://localhost:3000/', app), 'http://localhost:3000/')

  for (const value of [
    'about:blank',
    'javascript:alert(1)',
    'file:///etc/passwd',
    'data:text/html,hi',
    'not a url',
    '',
    null,
    42,
    {},
    `https://example.com/${'a'.repeat(2048)}`,
  ])
    assert.equal(sanitizeBrowserUrl(value), null, String(value).slice(0, 40))
  assert.equal(sanitizeBrowserTitle('  A\u0000B\nC  '), 'A B C')
  assert.equal(sanitizeBrowserTitle('x'.repeat(500)).length, 200)
  assert.equal(sanitizeBrowserTitle(undefined), '')
})

function browserState(urls, activeIndex = 0) {
  let state = createContextPages('browser')
  urls.forEach((url, index) => {
    if (index > 0) state = update(state, { type: 'add', kind: 'browser' })
    state = update(state, {
      type: 'browser',
      id: `page-${index + 1}`,
      url,
      draft: url,
      title: `T${index}`,
    })
  })
  return update(state, { type: 'browser-tab', id: `page-${activeIndex + 1}` })
}

test('each session saves only its browser tabs, cleaned, with the active one', () => {
  let state = browserState(
    [
      'https://a.example/?token=1#top',
      '',
      `${app}/_pisper/html-preview/tok/x.html`,
      'https://b.example/',
    ],
    3,
  )
  state = update(state, { type: 'add', kind: 'files' })
  state = update(state, { type: 'browser', id: 'page-1', agent: true })
  assert.deepEqual(browserTabsSnapshot(state, app), {
    active: 1,
    tabs: [
      { url: 'https://a.example/', title: 'T0' },
      { url: 'https://b.example/', title: 'T3' },
    ],
  })
  // 当前页签存不下（空白页）时指向第一个。
  assert.equal(browserTabsSnapshot(browserState(['https://a.example/', ''], 1), app).active, 0)
  assert.equal(browserTabsSnapshot(browserState(['', 'about:blank']), app), null)
  assert.equal(browserTabsSnapshot(createContextPages('files'), app), null)
})

test('saved tabs round-trip, keep the 40 most recent sessions and restore without Agent state', () => {
  const sessions = Array.from({ length: PERSISTED_BROWSER_SESSIONS + 5 }, (_, index) => ({
    id: `session-${index}`,
    updatedAt: 1_000 + index,
    active: 1,
    tabs: [
      { url: `https://s${index}.example/`, title: 'one' },
      { url: `https://s${index}.example/two`, title: 'two' },
    ],
  }))
  const raw = serializeBrowserTabs(sessions)
  const parsed = parseBrowserTabs(raw)
  assert.equal(parsed.length, PERSISTED_BROWSER_SESSIONS)
  assert.equal(parsed[0].id, `session-${PERSISTED_BROWSER_SESSIONS + 4}`)
  assert.equal(parsed.at(-1).id, 'session-5')
  assert.deepEqual(parsed[0].tabs[1], { url: `https://s44.example/two`, title: 'two' })
  assert.equal(serializeBrowserTabs([]), null)

  const restored = restoreBrowserPages(parsed[0].tabs, parsed[0].active)
  assert.equal(restored.pages.length, 2)
  assert.equal(restored.activeBrowserId, 'page-2')
  assert.equal(restored.activeId, 'page-2')
  assert.equal(restored.nextId, 3)
  assert.equal(restored.contentOpen, false)
  for (const page of restored.pages) {
    assert.equal(page.kind, 'browser')
    assert.equal(page.agent, false)
    assert.equal(page.resourceId, '')
    assert.equal(page.draft, page.url)
  }
  // 恢复后照常能加页签，不会和恢复出来的 id 撞上。
  const added = update(restored, { type: 'add', kind: 'files' })
  assert.equal(added.pages.at(-1).id, 'page-3')
  // 越界的当前页签退回第一个。
  assert.equal(restoreBrowserPages(parsed[0].tabs, 9).activeBrowserId, 'page-1')
})

test('saved tabs are validated as untrusted data and stay within the preference budget', () => {
  for (const raw of [
    null,
    '',
    '{',
    '[]',
    '{"version":2,"sessions":[]}',
    '{"version":1,"sessions":{}}',
  ])
    assert.deepEqual(parseBrowserTabs(raw), [], String(raw))
  const parsed = parseBrowserTabs(
    JSON.stringify({
      version: 1,
      sessions: [
        null,
        { id: '', tabs: [{ url: 'https://a.example/' }] },
        { id: 'bad\u0000id', tabs: [{ url: 'https://a.example/' }] },
        { id: 'empty', tabs: [{ url: 'javascript:alert(1)' }] },
        {
          id: 'ok',
          updatedAt: 'soon',
          active: 2,
          tabs: [
            { url: 'https://a.example/?token=x#f', title: 7 },
            'nope',
            { url: 'https://b.example/', title: 'B' },
            ...Array.from({ length: 20 }, (_, i) => ({ url: `https://c${i}.example/` })),
          ],
        },
        { id: 'ok', tabs: [{ url: 'https://dup.example/' }] },
      ],
    }),
  )
  assert.equal(parsed.length, 1)
  assert.equal(parsed[0].id, 'ok')
  assert.equal(parsed[0].updatedAt, 0)
  assert.equal(parsed[0].tabs.length, PERSISTED_BROWSER_TABS)
  assert.deepEqual(parsed[0].tabs[0], { url: 'https://a.example/', title: '' })
  // active 指向原数组第 3 项（b.example），清洗后在第 2 位。
  assert.equal(parsed[0].active, 1)

  const long = `https://example.com/${'p'.repeat(1900)}`
  const heavy = Array.from({ length: PERSISTED_BROWSER_SESSIONS }, (_, index) => ({
    id: `s${index}`,
    updatedAt: index,
    active: 0,
    tabs: Array.from({ length: PERSISTED_BROWSER_TABS }, () => ({
      url: long,
      title: '标题'.repeat(100),
    })),
  }))
  const text = serializeBrowserTabs(heavy)
  assert.ok(new TextEncoder().encode(text).byteLength <= PERSISTED_BROWSER_TABS_MAX_BYTES)
  const kept = parseBrowserTabs(text)
  assert.ok(kept.length > 0 && kept.length < heavy.length)
  // 丢的是最旧的。
  assert.equal(kept[0].id, `s${PERSISTED_BROWSER_SESSIONS - 1}`)
})

test('browsing history keeps one entry per address, newest first, capped and cleaned', () => {
  let entries = []
  entries = recordBrowserVisit(entries, 'https://a.example/#x', 'A', 100, app)
  entries = recordBrowserVisit(entries, 'https://b.example/', '', 200, app)
  entries = recordBrowserVisit(entries, 'https://a.example/', '', 300, app)
  assert.deepEqual(entries, [
    { url: 'https://a.example/', title: 'A', lastVisitedAt: 300, visits: 2 },
    { url: 'https://b.example/', title: '', lastVisitedAt: 200, visits: 1 },
  ])
  // 不记的地址原样返回同一个数组，Store 据此不写盘。
  for (const url of ['about:blank', `${app}/_pisper/html-preview/t/a.html`, `${app}/board`, ''])
    assert.equal(recordBrowserVisit(entries, url, 'x', 400, app), entries, url)
  const titled = updateBrowserHistoryTitle(entries, 'https://b.example/#y', 'B', app)
  assert.equal(titled[1].title, 'B')
  assert.equal(titled[1].visits, 1)
  assert.equal(updateBrowserHistoryTitle(titled, 'https://b.example/', 'B', app), titled)
  assert.equal(updateBrowserHistoryTitle(titled, 'https://missing.example/', 'M', app), titled)
  // 凭据样的参数不进历史，清洗后相同的地址合成一条。
  entries = recordBrowserVisit(entries, 'https://b.example/?token=secret', '', 500, app)
  assert.equal(entries[0].url, 'https://b.example/')
  assert.equal(entries[0].visits, 2)

  let many = []
  for (let i = 0; i < MAX_BROWSER_HISTORY + 20; i++)
    many = recordBrowserVisit(many, `https://h${i}.example/`, '', i, app)
  assert.equal(many.length, MAX_BROWSER_HISTORY)
  assert.equal(many[0].url, `https://h${MAX_BROWSER_HISTORY + 19}.example/`)
  assert.equal(many.at(-1).url, 'https://h20.example/')
})

test('browsing history round-trips, tolerates corrupt data and stays within budget', () => {
  const entries = [
    { url: 'https://a.example/', title: 'A', lastVisitedAt: 300, visits: 2 },
    { url: 'https://b.example/', title: 'B', lastVisitedAt: 200, visits: 1 },
  ]
  assert.deepEqual(parseBrowserHistory(serializeBrowserHistory(entries)), entries)
  assert.equal(serializeBrowserHistory([]), null)
  for (const raw of [null, '', 'oops', '{"version":1}', '{"version":3,"entries":[]}'])
    assert.deepEqual(parseBrowserHistory(raw), [], String(raw))
  assert.deepEqual(
    parseBrowserHistory(
      JSON.stringify({
        version: 1,
        entries: [
          { url: 'https://old.example/', title: 'Old', lastVisitedAt: 10, visits: -3 },
          { url: 'ftp://x.example/', lastVisitedAt: 50 },
          { url: 'https://new.example/#frag', title: 'New', lastVisitedAt: 90, visits: 2 },
          { url: 'https://new.example/', title: 'Older title', lastVisitedAt: 40, visits: 1.5 },
          'junk',
        ],
      }),
    ),
    [
      { url: 'https://new.example/', title: 'New', lastVisitedAt: 90, visits: 3 },
      { url: 'https://old.example/', title: 'Old', lastVisitedAt: 10, visits: 1 },
    ],
  )
  const heavy = Array.from({ length: MAX_BROWSER_HISTORY }, (_, i) => ({
    url: `https://example.com/${i}/${'p'.repeat(1900)}`,
    title: '标题'.repeat(100),
    lastVisitedAt: MAX_BROWSER_HISTORY - i,
    visits: 1,
  }))
  const text = serializeBrowserHistory(heavy)
  assert.ok(new TextEncoder().encode(text).byteLength <= BROWSER_HISTORY_MAX_BYTES)
  const kept = parseBrowserHistory(text)
  assert.ok(kept.length > 0 && kept.length < heavy.length)
  assert.equal(kept[0].url, heavy[0].url)
})

test('history groups by local day: today, yesterday, last 7 days, earlier', () => {
  const now = new Date(2026, 10, 8, 15, 0).getTime()
  const at = (day, hour = 12) => new Date(2026, 10, day, hour).getTime()
  const entry = (url, lastVisitedAt) => ({ url, title: '', lastVisitedAt, visits: 1 })
  const groups = groupBrowserHistory(
    [
      entry('https://t.example/', at(8, 0)),
      entry('https://y.example/', at(7, 23)),
      entry('https://w.example/', at(2)),
      entry('https://e.example/', at(1, 23)),
    ],
    now,
  )
  assert.deepEqual(
    groups.map((group) => [group.key, group.entries.map((item) => new URL(item.url).host)]),
    [
      ['today', ['t.example']],
      ['yesterday', ['y.example']],
      ['week', ['w.example']],
      ['earlier', ['e.example']],
    ],
  )
  assert.deepEqual(groupBrowserHistory([], now), [])
})

test('browser tabs and history persist through registered page-state keys only', async () => {
  assert.ok(BROWSER_PREFERENCE_KEYS.includes(SESSION_BROWSER_TABS_KEY))
  assert.ok(BROWSER_PREFERENCE_KEYS.includes(BROWSER_HISTORY_KEY))
  const [store, history, writer, pane, menu, selection] = await Promise.all([
    readFile('src/features/chat/session-context-pages-store.ts', 'utf8'),
    readFile('src/features/chat/browser-history-store.ts', 'utf8'),
    readFile('src/lib/debounced-page-state.ts', 'utf8'),
    readFile('src/features/chat/SessionBrowserPane.tsx', 'utf8'),
    readFile('src/features/chat/BrowserHistoryMenu.tsx', 'utf8'),
    readFile('src/features/chat/use-session-selection.ts', 'utf8'),
  ])
  // 只经 page-state-storage 写（键不在白名单会直接报错），并且合并写入，不是每次跳转都写。
  for (const source of [store, history]) {
    assert.doesNotMatch(source, /\b(?:local|session)Storage\b/)
    assert.match(source, /createDebouncedPageStateWriter\(/)
  }
  assert.match(writer, /pageStateStorage\.setItem\(key, value\)/)
  // 删除会话时页签记录跟着删：forget 之后排一次写入。
  assert.match(
    store,
    /forget: \(sessionId\) => \{[\s\S]*?delete sessions\[sessionId\][\s\S]*?persist\(\)/,
  )
  assert.match(selection, /useContextPagesStore\.getState\(\)\.forget\(id\)/)
  // 原生导航与 iframe 都记历史；历史入口是 Popover + Command 的可搜索列表，清除要确认。
  assert.match(pane, /recordVisit\(next\)/)
  assert.match(pane, /useBrowserHistoryStore\.getState\(\)\.visit\(url\)/)
  assert.match(pane, /<BrowserHistoryMenu onOpen=/)
  assert.match(menu, /<Popover open=\{open\}/)
  assert.match(menu, /<CommandInput/)
  assert.match(menu, /<InlineEmptyState\s+compact/)
  assert.match(menu, /<AlertDialog open=\{confirming\}/)
})
