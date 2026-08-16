import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

const COMPONENTS = [
  'AnimatedContent',
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

test('React Bits effects are lazy, CSS-owned, and preserve core UI fallbacks', async () => {
  const [
    focus,
    welcome,
    activity,
    confirmation,
    history,
    preview,
    main,
    shinyText,
    aurora,
    appStyles,
    bitsStyles,
  ] = await Promise.all([
    readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
    readFile('src/features/chat/WelcomeEffects.tsx', 'utf8'),
    readFile('src/features/chat/AgentRunActivity.tsx', 'utf8'),
    readFile('src/components/ai-elements/confirmation.tsx', 'utf8'),
    readFile('src/features/chat/ChatHistoryPage.tsx', 'utf8'),
    readFile('src/features/chat/WebPreviewDockPanel.tsx', 'utf8'),
    readFile('src/main.tsx', 'utf8'),
    readFile('src/components/react-bits/ShinyText.tsx', 'utf8'),
    readFile('src/components/react-bits/Aurora.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
    readFile('src/components/react-bits/react-bits.css', 'utf8'),
  ])
  assert.match(focus, /lazy\(\(\) => import\('\.\/WelcomeEffects'\)\)/)
  assert.match(focus, /fallback=\{<WelcomeFallback/)
  assert.match(focus, /data-target-cursor/)
  assert.match(welcome, /<Aurora \/>/)
  assert.match(welcome, /<AsciiText text="PISPER" \/>/)
  assert.match(welcome, /<BlurText/)
  assert.match(welcome, /<TargetCursor/)
  assert.match(activity, /import\('@\/components\/react-bits\/ShinyText'\)/)
  assert.match(activity, /import\('@\/components\/react-bits\/AnimatedList'\)/)
  assert.match(activity, /<Suspense fallback=\{activityCards\}>/)
  assert.match(activity, /<ActivityCard/)
  assert.match(confirmation, /import\('@\/components\/react-bits\/ClickSpark'\)/)
  assert.match(confirmation, /<Suspense fallback=\{action\}>/)
  assert.match(history, /from '@\/components\/react-bits\/SpotlightCard'/)
  assert.match(preview, /import\('@\/components\/react-bits\/Threads'\)/)
  assert.match(preview, /<Suspense fallback=\{null\}>/)
  assert.doesNotMatch(main, /react-bits\.css/)
  assert.match(shinyText, /import '\.\/react-bits\.css'/)
  assert.match(focus, /agent-welcome-content[^"\n]*:root\[data-theme='light'\]/)
  assert.match(aurora, /rb-aurora[^"\n]*:root\[data-theme='light'\]/)

  const auroraRule = bitsStyles.match(/\.rb-aurora i \{([^}]+)\}/)?.[1] || ''
  assert.match(auroraRule, /radial-gradient/)
  assert.doesNotMatch(auroraRule, /filter|mix-blend-mode|animation/)
  assert.doesNotMatch(appStyles, /\.agent-welcome \.welcome-logo \.logo-star \{[^}]*animation/)
  assert.doesNotMatch(bitsStyles, /\.rb-shiny-text \{[^}]*animation:[^;}]*infinite/)
})
