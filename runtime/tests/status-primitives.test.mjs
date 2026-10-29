import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { StatusGlyph } from '../../src/components/app/StatusGlyph.tsx'
import { StatusTag } from '../../src/components/app/StatusTag.tsx'
import {
  STATUS_GLYPH_KINDS,
  STATUS_GLYPH_TONES,
} from '../../src/components/app/status-glyph-kinds.ts'
import { Chip } from '../../src/components/ui/chip.tsx'
import { SegmentedControl } from '../../src/components/ui/segmented-control.tsx'

test('every status has its own shape, drawn in currentColor and decorative unless labelled', () => {
  const shapes = new Set()
  for (const status of STATUS_GLYPH_KINDS) {
    const html = renderToStaticMarkup(h(StatusGlyph, { status }))
    assert.match(html, new RegExp(`data-status-glyph="${status}"`))
    assert.match(html, /aria-hidden="true"/)
    assert.doesNotMatch(html, /#[\da-f]{3,8}\b|rgba?\(/i, `${status} must not hard-code a color`)
    shapes.add(html.replace(/data-status-glyph="[^"]+"/, '').replace(/class="[^"]*"/, ''))
  }
  assert.equal(shapes.size, STATUS_GLYPH_KINDS.length, 'status glyph shapes must be distinct')

  const labelled = renderToStaticMarkup(h(StatusGlyph, { status: 'failed', label: '失败' }))
  assert.match(labelled, /role="img"/)
  assert.match(labelled, /aria-label="失败"/)
  assert.doesNotMatch(labelled, /aria-hidden/)
})

test('only the running glyph spins and reduced motion stops it', () => {
  const running = renderToStaticMarkup(h(StatusGlyph, { status: 'run' }))
  assert.match(running, /animate-\[spin_1\.1s_linear_infinite\]/)
  // 静态 HTML 会把 & 转义为 &amp;。
  assert.match(running, /\[:root\[data-motion=reduced\]_&amp;\]:animate-none/)
  assert.match(running, /motion-reduce:\[:root:not\(\[data-motion=full\]\)_&amp;\]:animate-none/)
  assert.doesNotMatch(renderToStaticMarkup(h(StatusGlyph, { status: 'run', spin: false })), /spin_/)
  for (const status of STATUS_GLYPH_KINDS.filter((kind) => kind !== 'run'))
    assert.doesNotMatch(renderToStaticMarkup(h(StatusGlyph, { status })), /spin_/, status)
})

test('status tags color text, soft fill and border from the status family only', () => {
  assert.deepEqual(STATUS_GLYPH_TONES, {
    run: 'run',
    approval: 'attn',
    question: 'attn',
    failed: 'fail',
    queued: 'queue',
    done: 'done',
    stopped: 'queue',
  })
  for (const status of STATUS_GLYPH_KINDS) {
    const tone = STATUS_GLYPH_TONES[status]
    const html = renderToStaticMarkup(h(StatusTag, { status }, '状态'))
    assert.match(html, new RegExp(`data-tone="${tone}"`))
    for (const utility of [`text-st-${tone}`, `bg-st-${tone}-soft`, `border-st-${tone}-border`])
      assert.ok(html.includes(utility), `${status} tag uses ${utility}`)
    assert.match(html, /h-\[22px\]/)
    assert.doesNotMatch(html, /star|accent/, `${status} tag must not follow the accent color`)
  }
  assert.match(
    renderToStaticMarkup(h(StatusTag, { status: 'failed', tone: 'queue' }, '已查看')),
    /data-tone="queue"/,
  )
})

test('segmented control is a single-choice radio group and chips are non-submitting buttons', () => {
  const segmented = renderToStaticMarkup(
    h(SegmentedControl, {
      'aria-label': '项目',
      value: 'all',
      onValueChange() {},
      options: [
        { value: 'all', label: '全部', count: 8 },
        { value: 'pisper', label: 'pisper', count: 3, disabled: true },
      ],
    }),
  )
  assert.match(segmented, /role="radiogroup"/)
  assert.match(segmented, /aria-label="项目"/)
  assert.equal(segmented.match(/role="radio"/g)?.length, 2)
  assert.match(segmented, /aria-checked="true"[^>]*>全部/)
  assert.match(segmented, /disabled=""/)
  assert.match(segmented, /tabular-nums[^>]*>8</)

  const chip = renderToStaticMarkup(h(Chip, { 'aria-pressed': true }, 'Plan 模式'))
  assert.match(chip, /^<button/)
  assert.match(chip, /type="button"/)
  assert.match(chip, /aria-pressed="true"/)
})

test('status primitives stay free of feature imports', async () => {
  for (const file of [
    'components/app/StatusGlyph.tsx',
    'components/app/StatusTag.tsx',
    'components/app/status-glyph-kinds.ts',
    'components/ui/chip.tsx',
    'components/ui/segmented-control.tsx',
  ]) {
    const source = await readFile(new URL(`../../src/${file}`, import.meta.url), 'utf8')
    assert.doesNotMatch(source, /from ['"]@\/(?:features|app|stores)\//, file)
  }
})
