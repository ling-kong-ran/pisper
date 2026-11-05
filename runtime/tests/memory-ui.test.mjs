import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { groupMemoriesByTime } from '../../src/features/memory/memory-timeline.ts'

const read = (file) => readFile(`src/features/memory/${file}`, 'utf8')

test('the memory page opens on the lazily loaded evolution view, with timeline and activity beside it', async () => {
  const page = await read('MemoryPage.tsx')
  // 画布和动画不进页面主包。
  assert.match(page, /const MemoryEvolution = lazy\(\(\) => import\('\.\/MemoryEvolution'\)\)/)
  assert.doesNotMatch(page, /from '\.\/MemoryEvolution'/)
  assert.match(page, /stored === 'timeline' \|\| stored === 'activity' \? stored : 'evolution'/)
  assert.match(page, /<FeaturePage\b/)
  // FeaturePage 要是 .page-content 的直接子元素，Tabs 只能合并上去，不能多包一层。
  assert.match(page, /<Tabs asChild\b/)
  assert.match(page, /id=\{FEATURE_SEARCH_INPUT_ID\}/)
  for (const view of ['evolution', 'timeline', 'activity'])
    assert.match(page, new RegExp(`<TabsTrigger value="${view}">`))
  // 选中的视图经 page-state-storage 持久化，键已登记。
  assert.match(page, /pageStateStorage\.setItem\(STORAGE_KEYS\.memoryView/)
  const preferences = await readFile('shared/browser-preferences.mjs', 'utf8')
  assert.match(preferences, /'pisper-memory-view'/)
})

test('the memory page goes through the domain API, uses current controls and has no review queue', async () => {
  const files = await Promise.all(
    ['MemoryPage.tsx', 'MemoryDetailPanel.tsx', 'MemoryDialogs.tsx', 'MemoryActivity.tsx'].map(
      read,
    ),
  )
  for (const source of files) {
    assert.doesNotMatch(source, /<select\b/)
    assert.doesNotMatch(source, /\bapiJson\b|\bfetch\(/)
    assert.doesNotMatch(source, /modal-backdrop|fixed inset-0/)
    assert.doesNotMatch(source, /candidate|autoApprove|threshold/i)
  }
  const [page, panel, dialogs, activity] = files
  assert.match(page, /<AppSelect\b/)
  assert.match(page, /<DropdownMenu>/)
  assert.match(page, /<SegmentedControl<TimelineStatus>/)
  assert.match(dialogs, /<Dialog open busy=\{saving\}/)
  // 详情由时间线展开和演算视图共用同一个组件。
  assert.equal(page.match(/<MemoryDetailPanel\b/g)?.length, 2)
  assert.match(page, /data-memory-detail/)
  assert.match(panel, /fetchMemoryDetail\(id, controller\.signal\)/)
  // 删除要说清会连同历史版本一起彻底删除。
  assert.match(panel, /deleteMemoryName/)
  // 连到旧 Runtime 没有整理记录接口时不算错误。
  assert.match(activity, /loadError\.status === 404\) setUnsupported\(true\)/)
})

test('a late search response cannot overwrite a newer one', async () => {
  const page = await read('MemoryPage.tsx')
  assert.match(page, /const revision = \+\+loadRevision\.current/)
  assert.match(page, /if \(revision !== loadRevision\.current\) return/)
})

test('the delete confirmation promises what the runtime does', async () => {
  const zh = JSON.parse(await readFile('src/locales/zh-CN/memory.json', 'utf8'))
  assert.match(zh['memoryPage.deleteMemoryName'], /历史版本/)
  assert.match(zh['memoryPage.lede'], /拖动时间轴/)
})

test('the timeline groups memories into today, yesterday, this week, then months', () => {
  // 2026-11-05 是周四。
  const now = new Date(2026, 10, 5, 12).getTime()
  const at = (month, day) => new Date(2026, month, day, 9).toISOString()
  const groups = groupMemoriesByTime(
    [
      { id: 'future', at: at(10, 9) },
      { id: 'today', at: at(10, 5) },
      { id: 'yesterday', at: at(10, 4) },
      { id: 'week', at: at(10, 2) },
      { id: 'last-week', at: at(9, 30) },
      { id: 'october', at: at(9, 2) },
      { id: 'september', at: at(8, 20) },
      { id: 'broken', at: 'not a date' },
    ],
    (item) => item.at,
    now,
  )
  assert.deepEqual(
    groups.map((group) => [group.kind, group.items.map((item) => item.id)]),
    [
      ['today', ['future', 'today']],
      ['yesterday', ['yesterday']],
      ['week', ['week']],
      ['month', ['last-week', 'october']],
      ['month', ['september']],
      ['unknown', ['broken']],
    ],
  )
  assert.equal(groups[3].key, '2026-10')
  assert.equal(groups[3].month.getMonth(), 9)
})

test('on a Monday this week is just today and yesterday falls in last week’s month group or yesterday', () => {
  const monday = new Date(2026, 10, 2, 12).getTime()
  const groups = groupMemoriesByTime(
    [
      { at: new Date(2026, 10, 2, 8).toISOString() },
      { at: new Date(2026, 10, 1, 8).toISOString() },
      { at: new Date(2026, 9, 31, 8).toISOString() },
    ],
    (item) => item.at,
    monday,
  )
  assert.deepEqual(
    groups.map((group) => group.kind),
    ['today', 'yesterday', 'month'],
  )
})

test('the time axis stays above the fold, whatever the page header measures', async () => {
  const stage = await readFile('src/features/memory/MemoryEvolution.tsx', 'utf8')
  // 舞台高度量真实可用空间，不能写死页头高度：页头一变，时间轴就被挤出首屏。
  assert.match(stage, /scroller\.clientHeight - top - STAGE_BOTTOM_GAP/)
})

test('activity entries for consolidation read as sentences, not machine codes', async () => {
  const labels = await readFile('src/features/memory/memory-labels.ts', 'utf8')
  assert.match(labels, /consolidate: \{/)
  assert.match(labels, /digests:\(\\d\+\) merged:\(\\d\+\)/)
})
