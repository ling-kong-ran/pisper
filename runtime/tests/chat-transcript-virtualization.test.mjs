import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { Virtualizer } from '@tanstack/react-virtual'
import {
  anchoredScrollTopAfterPrepend,
  TRANSCRIPT_ESTIMATED_ROW_HEIGHT,
  TRANSCRIPT_OVERSCAN,
} from '../../src/features/chat/transcript-virtualization.ts'

function transcriptVirtualizer(sizes, initialOffset = 0) {
  const getItemKey = (index) => `message-${index}`
  const virtualizer = new Virtualizer({
    count: sizes.length,
    estimateSize: () => TRANSCRIPT_ESTIMATED_ROW_HEIGHT,
    getItemKey,
    getScrollElement: () => null,
    initialOffset,
    initialRect: { width: 390, height: 844 },
    observeElementOffset() {},
    observeElementRect() {},
    overscan: TRANSCRIPT_OVERSCAN,
    scrollToFn() {},
  })
  virtualizer.getTotalSize()
  sizes.forEach((size, index) => virtualizer.resizeItem(index, size))
  return virtualizer
}

test('one thousand mixed-height transcript messages keep a bounded rendered window', () => {
  const sizes = Array.from({ length: 1_000 }, (_, index) => 60 + (index % 9) * 37)
  const virtualizer = transcriptVirtualizer(sizes, 60_000)
  const rendered = virtualizer.getVirtualItems()

  assert.equal(
    virtualizer.getTotalSize(),
    sizes.reduce((total, size) => total + size, 0),
  )
  assert.ok(rendered.length > 0)
  assert.ok(rendered.length < 32, `expected a bounded range, rendered ${rendered.length}`)
  assert.ok(rendered[0].index > 0)
  assert.ok(rendered.at(-1).index < sizes.length - 1)
})

test('streaming append and growth remeasure the live transcript row', () => {
  const sizes = Array.from({ length: 20 }, (_, index) => 72 + (index % 4) * 48)
  const virtualizer = transcriptVirtualizer(sizes)
  const beforeAppend = virtualizer.getTotalSize()

  sizes.push(TRANSCRIPT_ESTIMATED_ROW_HEIGHT)
  virtualizer.setOptions({ ...virtualizer.options, count: sizes.length })
  assert.equal(virtualizer.getTotalSize(), beforeAppend + TRANSCRIPT_ESTIMATED_ROW_HEIGHT)

  const liveIndex = sizes.length - 1
  const grownHeight = TRANSCRIPT_ESTIMATED_ROW_HEIGHT + 356
  virtualizer.resizeItem(liveIndex, grownHeight)
  assert.equal(
    virtualizer.getTotalSize(),
    beforeAppend + grownHeight,
    'the virtual canvas must grow with streaming Markdown and expanded code blocks',
  )
  assert.equal(virtualizer.measurementsCache[liveIndex].size, grownHeight)
})

test('history prepend anchor math preserves the visible transcript offset', () => {
  const snapshot = { scrollHeight: 5_000, scrollTop: 240 }
  const nextScrollHeight = 7_360
  const nextScrollTop = anchoredScrollTopAfterPrepend(snapshot, nextScrollHeight)

  assert.equal(nextScrollTop, 2_600)
  assert.equal(nextScrollHeight - nextScrollTop, snapshot.scrollHeight - snapshot.scrollTop)
})

test('virtualization source owns only message rows and preserves stable render boundaries', async () => {
  const [
    virtualSource,
    transcriptSource,
    sessionSource,
    messageSource,
    activitySource,
    autoScrollSource,
    packageJson,
  ] = await Promise.all([
    readFile('src/features/chat/VirtualMessageTranscript.tsx', 'utf8'),
    readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/ChatMessage.tsx', 'utf8'),
    readFile('src/features/chat/AgentRunActivity.tsx', 'utf8'),
    readFile('src/hooks/useAutoScroll.ts', 'utf8'),
    readFile('package.json', 'utf8').then(JSON.parse),
  ])

  assert.equal(packageJson.dependencies['@tanstack/react-virtual'], undefined)
  assert.ok(packageJson.devDependencies['@tanstack/react-virtual'])
  assert.match(virtualSource, /useVirtualizer<HTMLDivElement, HTMLDivElement>/)
  assert.match(virtualSource, /getScrollElement: \(\) => scrollElement/)
  assert.match(virtualSource, /messagesRef\.current\[index\]\?\.id/)
  assert.match(virtualSource, /measureElement: measuredElementHeight/)
  assert.match(virtualSource, /new ResizeObserver\(measure\)/)
  assert.match(virtualSource, /onContentSizeChange\(\)/)
  assert.match(virtualSource, /virtualItems\.map/)
  assert.doesNotMatch(
    virtualSource,
    /PlanBoard|ToolApproval|focus-composer|history-page-loader|chat-error/,
  )

  const virtualBoundary = transcriptSource.indexOf('<VirtualMessageTranscript')
  assert.ok(transcriptSource.indexOf('history-page-loader') < virtualBoundary)
  assert.doesNotMatch(transcriptSource, /PlanBoard|plan-board-dock/)
  assert.ok(transcriptSource.indexOf('{error && (') > virtualBoundary)
  assert.doesNotMatch(sessionSource, /PlanBoard|plan-board-dock/)
  assert.ok(
    sessionSource.indexOf('<FocusTranscript') < sessionSource.indexOf('focus-composer-shell'),
  )
  assert.ok(sessionSource.indexOf('<ToolApproval') > sessionSource.indexOf('<FocusTranscript'))
  assert.match(transcriptSource, /ref=\{setTranscriptRef\}/)
  assert.match(transcriptSource, /scrollElement=\{transcriptElement\}/)
  assert.match(transcriptSource, /onContentSizeChange=\{maintainBottom\}/)
  assert.match(transcriptSource, /onPointerDown=\{cancelProgrammaticScroll\}/)
  assert.match(transcriptSource, /onTouchStart=\{cancelProgrammaticScroll\}/)
  assert.match(transcriptSource, /onWheel=\{cancelProgrammaticScroll\}/)
  assert.match(autoScrollSource, /setScrollElement\(node\)/)
  assert.match(autoScrollSource, /const pinnedToBottomRef = useRef\(true\)/)
  assert.match(autoScrollSource, /if \(pinnedToBottomRef\.current\) scrollToBottom\(\)/)
  assert.match(autoScrollSource, /programmaticScrollRef\.current = false/)

  assert.match(messageSource, /memo\(function FocusChatMessage[\s\S]*focusPropsEqual\)/)
  assert.match(activitySource, /const ActivityCard = memo\(function ActivityCard/)
  assert.match(activitySource, /activityCardPropsEqual/)
  assert.match(activitySource, /memo\(AgentRunActivity, agentRunActivityPropsEqual\)/)
})
