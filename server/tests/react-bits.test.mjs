import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

const COMPONENTS = [
  'AnimatedList',
  'AsciiText',
  'Aurora',
  'BlurText',
  'ClickSpark',
  'ShinyText',
  'SpotlightCard',
  'TargetCursor',
  'Threads',
]

test('Pisper ships the selected lightweight React Bits components without another animation runtime', async () => {
  const [index, styles, packageJson, asciiText, targetCursor] = await Promise.all([
    readFile('src/components/react-bits/index.ts', 'utf8'),
    readFile('src/components/react-bits/react-bits.css', 'utf8'),
    readFile('package.json', 'utf8'),
    readFile('src/components/react-bits/AsciiText.tsx', 'utf8'),
    readFile('src/components/react-bits/TargetCursor.tsx', 'utf8'),
  ])
  for (const component of COMPONENTS) assert.match(index, new RegExp(`export \\{ ${component} \\}`))
  assert.match(styles, /@media \(prefers-reduced-motion: reduce\)/)
  assert.match(styles, /\.rb-shiny-text/)
  assert.match(styles, /\.rb-aurora/)
  assert.match(styles, /\.rb-ascii-text/)
  assert.match(styles, /\.rb-target-cursor/)
  assert.match(styles, /\.rb-threads/)
  assert.doesNotMatch(`${asciiText}\n${targetCursor}`, /gsap|ogl|three/)
  assert.match(asciiText, /ResizeObserver/)
  assert.doesNotMatch(asciiText, /requestAnimationFrame|setInterval/)
  assert.match(targetCursor, /pointer: fine/)
  assert.equal(JSON.parse(packageJson).devDependencies.motion, '^12.42.2')
})

test('React Bits effects are attached to purposeful low-noise UI surfaces', async () => {
  const [focus, activity, confirmation, history, preview, appStyles, bitsStyles] = await Promise.all([
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/AgentRunActivity.tsx', 'utf8'),
    readFile('src/components/ai-elements/confirmation.tsx', 'utf8'),
    readFile('src/features/chat/ChatHistoryPage.tsx', 'utf8'),
    readFile('src/features/chat/WebPreviewDockPanel.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
    readFile('src/components/react-bits/react-bits.css', 'utf8'),
  ])
  assert.match(focus, /<Aurora \/>/)
  assert.match(focus, /<AsciiText text="PISPER" \/>/)
  assert.match(focus, /<BlurText/)
  assert.match(focus, /<TargetCursor/)
  assert.match(focus, /data-target-cursor/)
  assert.match(activity, /<ShinyText>/)
  assert.match(activity, /<AnimatedList>/)
  assert.match(confirmation, /<ClickSpark>/)
  assert.match(history, /<SpotlightCard/)
  assert.match(preview, /<Threads \/>/)
  assert.match(appStyles, /:root\[data-theme='light'\] \.agent-welcome-content/)
  assert.match(appStyles, /:root\[data-theme='light'\] \.agent-welcome \.rb-aurora i/)

  const auroraRule = bitsStyles.match(/\.rb-aurora i \{([^}]+)\}/)?.[1] || ''
  assert.match(auroraRule, /radial-gradient/)
  assert.doesNotMatch(auroraRule, /filter|mix-blend-mode|animation/)
  assert.doesNotMatch(appStyles, /\.agent-welcome \.welcome-logo \.logo-star \{[^}]*animation/)
  assert.doesNotMatch(bitsStyles, /\.rb-shiny-text \{[^}]*animation:[^;}]*infinite/)
})
