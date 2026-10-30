import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

import { normalizeTokenUsage } from '../../src/lib/format.ts'

test('status bar usage normalization tolerates incomplete mobile runtime responses', () => {
  assert.deepEqual(normalizeTokenUsage(undefined), {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    reasoning: 0,
    totalTokens: 0,
  })
  assert.deepEqual(normalizeTokenUsage({ input: 12, output: 5 }), {
    input: 12,
    output: 5,
    cacheRead: 0,
    cacheWrite: 0,
    reasoning: 0,
    totalTokens: 17,
  })
})

test('status bar usage normalization accepts numeric strings and rejects invalid values', () => {
  assert.deepEqual(
    normalizeTokenUsage({
      input: '1,000',
      output: '20',
      cacheRead: -4,
      cacheWrite: 3,
      reasoning: Number.POSITIVE_INFINITY,
      total: '30',
    }),
    {
      input: 0,
      output: 20,
      cacheRead: 0,
      cacheWrite: 3,
      reasoning: 0,
      totalTokens: 30,
    },
  )
})

test('Pisper shell waits for client detection and retires the global status bar', async () => {
  const app = await readFile('src/App.tsx', 'utf8')

  assert.match(app, /const clientLoaded = useClientStore\(\(state\) => state\.loaded\)/)
  assert.doesNotMatch(app, /<StatusBar|import .*StatusBar/)
  assert.doesNotMatch(app, /pluginStats=\{pluginStats\}/)
  // 早已不挂载的全局状态栏在 v2（A1）随旧外壳删除；今日用量改由 useFleetSystemInfo 按需读取。
  await assert.rejects(readFile('src/components/layout/StatusBar.tsx', 'utf8'), { code: 'ENOENT' })
})

test('today usage is fetched only on demand once the fleet popover is retired', async () => {
  const systemInfo = await readFile('src/features/board/useFleetSystemInfo.ts', 'utf8')
  // 全局状态栏与标题栏舰队计量都已退役；“子代理 x/4 · 今日 tok · 内存”按规格 9.1 移到全部代理页头的
  // 悬停提示（A3）。系统信息沿用同一个容错归一化，并且只在调用方打开时请求（enabled）。
  assert.match(systemInfo, /apiJson<unknown>\('\/api\/usage\/today', \{ signal \}\)/)
  assert.match(systemInfo, /normalizeTokenUsage\(usage\)\.totalTokens/)
  assert.match(systemInfo, /export function useFleetSystemInfo\(enabled: boolean\)/)
  assert.match(
    systemInfo,
    /queryFn: \(\{ signal \}\) => fetchFleetSystemInfo\(signal\),\s*enabled,/,
  )
})
