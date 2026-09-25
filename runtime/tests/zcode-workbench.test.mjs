import assert from 'node:assert/strict'
import test from 'node:test'
import { readFile } from 'node:fs/promises'
import {
  greetingPeriod,
  nextGreetingDelay,
  greetingFontSize,
} from '../../src/features/chat/workbench-greeting.ts'

const at = (hour, minute = 0, second = 0) => new Date(2026, 8, 25, hour, minute, second)

test('ZCode greeting changes exactly at each local time boundary', () => {
  for (const [hour, expected] of [
    [0, 5],
    [4, 5],
    [5, 0],
    [8, 0],
    [9, 1],
    [11, 1],
    [12, 2],
    [13, 2],
    [14, 3],
    [17, 3],
    [18, 4],
    [22, 4],
    [23, 5],
  ]) {
    assert.equal(greetingPeriod(at(hour)), expected, `hour ${hour}`)
  }
  assert.equal(nextGreetingDelay(at(8, 59, 59)), 1000)
  assert.equal(nextGreetingDelay(at(9)), 3 * 60 * 60 * 1000)
  assert.equal(nextGreetingDelay(at(23)), 6 * 60 * 60 * 1000)
  assert.equal(nextGreetingDelay(at(23, 59, 59)), 5 * 60 * 60 * 1000 + 1000)
})

test('ZCode greeting font measurement remains bounded and tolerates hidden containers', () => {
  assert.equal(greetingFontSize(600, 300), 30)
  assert.equal(greetingFontSize(240, 300), 24)
  assert.equal(greetingFontSize(80, 300), 20)
  for (const [width, natural] of [
    [0, 0],
    [-1, 300],
    [300, 0],
    [NaN, 300],
    [300, Infinity],
  ]) {
    assert.equal(greetingFontSize(width, natural), 30)
  }
})

test('ZCode shell has one chat header, no split selector, and retains optional backend tools', async () => {
  const app = await readFile('src/App.tsx', 'utf8')
  const focus = await readFile('src/features/chat/FocusSession.tsx', 'utf8')
  const sidebar = await readFile('src/components/layout/AppSidebar.tsx', 'utf8')
  assert.doesNotMatch(app, /<ChatLayoutSwitcher|<ChatLayoutNavigation|<StatusBar/)
  assert.match(app, /page !== 'chat' &&/)
  assert.match(app, /requestSessionCreation\(''\)/)
  assert.equal(app.match(/onNewChat=\{startNewChat\}/g)?.length, 2)
  assert.match(focus, /<SidebarTrigger/)
  assert.match(focus, /<QuickPromptIdeas/)
  assert.match(focus, /<ExecutionModeSelect/)
  assert.match(sidebar, /<SidebarRecentSessions/)
  assert.match(sidebar, /<SidebarMoreTools/)
  assert.match(sidebar, /runtimeFeatureAvailable/)
})
