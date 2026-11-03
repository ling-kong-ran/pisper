import assert from 'node:assert/strict'
import { access, readdir, readFile } from 'node:fs/promises'
import test from 'node:test'

// 只保留仍在用的两个 React Bits 组件：AnimatedList（运行活动卡片）与 ClickSpark（审批确认）。
// 欢迎页的装饰动画（Aurora、AsciiText、ShinyText、TargetCursor 等）随欢迎页一起退役。
const KEPT = ['AnimatedList.tsx', 'ClickSpark.tsx', 'react-bits.css']

test('Pisper ships only the React Bits components it still renders', async () => {
  const [files, styles, packageJson] = await Promise.all([
    readdir('src/components/react-bits'),
    readFile('src/components/react-bits/react-bits.css', 'utf8'),
    readFile('package.json', 'utf8'),
  ])
  assert.deepEqual([...files].sort(), [...KEPT].sort())
  await assert.rejects(access('src/components/react-bits/index.ts'))
  assert.match(styles, /\.rb-animated-list-item/)
  assert.match(styles, /\.rb-click-spark-burst i/)
  assert.match(styles, /@media \(prefers-reduced-motion: reduce\)[\s\S]*\.rb-click-spark-burst i/)
  assert.doesNotMatch(styles, /rb-aurora|rb-ascii-text|rb-shiny-text|rb-target-cursor|rb-threads/)
  assert.equal(JSON.parse(packageJson).devDependencies.motion, '^12.42.2')
})

test('React Bits effects are lazy, CSS-owned, and preserve core UI fallbacks', async () => {
  const [activity, confirmation, history, composerBits, main, bitsStyles] = await Promise.all([
    readFile('src/features/chat/AgentRunActivity.tsx', 'utf8'),
    readFile('src/components/ai-elements/confirmation.tsx', 'utf8'),
    readFile('src/features/chat/ChatHistoryPage.tsx', 'utf8'),
    readFile('src/features/chat/focus-session-composer-bits.tsx', 'utf8'),
    readFile('src/main.tsx', 'utf8'),
    readFile('src/components/react-bits/react-bits.css', 'utf8'),
  ])
  assert.match(activity, /import\('@\/components\/react-bits\/AnimatedList'\)/)
  assert.match(activity, /<Suspense fallback=\{activityCards\}>/)
  assert.match(activity, /<ActivityCard/)
  assert.match(confirmation, /import\('@\/components\/react-bits\/ClickSpark'\)/)
  assert.match(confirmation, /<Suspense fallback=\{action\}>/)
  // Quiet Precision 不使用光泽、聚光与动态纹理：运行条文案、历史会话行保持静态。
  assert.doesNotMatch(composerBits, /react-bits\//)
  assert.doesNotMatch(history, /react-bits\//)
  assert.doesNotMatch(main, /react-bits\.css/)
  assert.doesNotMatch(bitsStyles, /animation:[^;}]*infinite/)
})
