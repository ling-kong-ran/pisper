import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  clearComposerDraft,
  readComposerDraft,
  updateComposerDraft,
} from '../../src/features/chat/composer-drafts.ts'
import {
  COMPOSER_INPUT_MAX_HEIGHT,
  fitComposerInputHeight,
} from '../../src/features/chat/composer-input-height.ts'

test('composer drafts retain text and attachments independently per session', () => {
  const attachment = { id: 'attachment-1', kind: 'path', path: '/workspace/notes.md' }
  updateComposerDraft('session-a', { text: 'first draft', attachments: [attachment] })
  updateComposerDraft('session-b', { text: 'second draft' })

  assert.deepEqual(readComposerDraft('session-a'), {
    text: 'first draft',
    attachments: [attachment],
  })
  assert.deepEqual(readComposerDraft('session-b'), { text: 'second draft', attachments: [] })

  clearComposerDraft('session-a')
  assert.deepEqual(readComposerDraft('session-a'), { text: '', attachments: [] })
  assert.deepEqual(readComposerDraft('session-b'), { text: 'second draft', attachments: [] })
  clearComposerDraft('session-b')
})

test('empty composer drafts are removed after successful submission', () => {
  updateComposerDraft('session-submit', { text: 'send this' })
  updateComposerDraft('session-submit', { text: '' })

  assert.deepEqual(readComposerDraft('session-submit'), { text: '', attachments: [] })
})

test('composer input height follows content, caps at the max and skips unlaid-out inputs', () => {
  // 先写 auto 再读 scrollHeight：用 getter 记下读的时候 height 已经是 auto。
  const input = (scrollHeight) => {
    const element = { style: { height: '220px' }, heightWhenMeasured: '' }
    Object.defineProperty(element, 'scrollHeight', {
      get() {
        element.heightWhenMeasured = element.style.height
        return scrollHeight
      },
    })
    return element
  }
  const short = input(76)
  fitComposerInputHeight(short)
  assert.equal(short.heightWhenMeasured, 'auto')
  assert.equal(short.style.height, '76px')

  const long = input(416)
  fitComposerInputHeight(long)
  assert.equal(long.style.height, `${COMPOSER_INPUT_MAX_HEIGHT}px`)

  // display: none 时量不出高度：留空给 CSS 最小高度，不能写成 0px。
  const hidden = input(0)
  fitComposerInputHeight(hidden)
  assert.equal(hidden.style.height, '')
})

test('the session composer re-measures its height whenever the draft value changes', async () => {
  // 回归：长草稿离开页面再回来（或切换会话）从缓存恢复时没有 onChange，只在输入时量高度会
  // 留下 44px 的单行高度，长文字挤在两行里，删光重输才恢复。
  const source = await readFile(
    new URL('../../src/features/chat/FocusSession.tsx', import.meta.url),
    'utf8',
  )
  assert.match(
    source,
    /useLayoutEffect\(\(\) => \{\s*if \(promptRef\.current\) fitComposerInputHeight\(promptRef\.current\)\s*\}, \[value\]\)/,
  )
  assert.doesNotMatch(source, /style\.height = `\$\{Math\.min\(/)
})
