import assert from 'node:assert/strict'
import test from 'node:test'
import {
  parseCanvasCss,
  CANVAS_CSS_MAX_LENGTH,
  splitCanvasCssDeclarations,
} from '../../src/features/chat/layout/chat-canvas-style.ts'

test('node CSS supports common layout, typography, variables and gradients without a DOM', () => {
  assert.deepEqual(
    parseCanvasCss(`
    display: grid;
    grid-template-columns: 1fr minmax(240px, 2fr);
    gap: clamp(8px, 2vw, 24px);
    padding-inline: 1rem;
    border-radius: 20px;
    background: linear-gradient(135deg, #fff, rgb(230 240 250 / .8));
    box-shadow: 0 4px 24px hsl(220 20% 20% / .1);
    font-family: "Noto Serif", serif;
    color: var(--text);
    --chat-user-color: oklch(.4 .1 250);
    -webkit-line-clamp: 3;
  `),
    {
      display: 'grid',
      gridTemplateColumns: '1fr minmax(240px, 2fr)',
      gap: 'clamp(8px, 2vw, 24px)',
      paddingInline: '1rem',
      borderRadius: '20px',
      background: 'linear-gradient(135deg, #fff, rgb(230 240 250 / .8))',
      boxShadow: '0 4px 24px hsl(220 20% 20% / .1)',
      fontFamily: '"Noto Serif", serif',
      color: 'var(--text)',
      '--chat-user-color': 'oklch(.4 .1 250)',
      WebkitLineClamp: '3',
    },
  )
})

test('declarations preserve quoted separators and custom property case, with later values winning', () => {
  assert.deepEqual(
    parseCanvasCss('font-family: "A; B:C", serif; color: red; COLOR: blue; --MyColor: #fff;'),
    {
      fontFamily: '"A; B:C", serif',
      color: 'blue',
      '--MyColor': '#fff',
    },
  )
  assert.deepEqual(parseCanvasCss('grid-template-areas: "header header" "main aside";'), {
    gridTemplateAreas: '"header header" "main aside"',
  })
  assert.deepEqual(parseCanvasCss(' ; \n\t ; '), {})
})

test('editor declaration splitting preserves quoted separators and validates before rewriting', () => {
  const text = ' font-family: "A; color: red", serif;\ncolor: blue; --MyColor: #fff; ; '
  assert.deepEqual(splitCanvasCssDeclarations(text), [
    'font-family: "A; color: red", serif',
    'color: blue',
    '--MyColor: #fff',
  ])
  const replaced = splitCanvasCssDeclarations(text).filter(
    (declaration) => declaration.split(':', 1)[0].trim().toLowerCase() !== 'color',
  )
  replaced.push('color: teal')
  assert.deepEqual(parseCanvasCss(replaced.join(';')), {
    fontFamily: '"A; color: red", serif',
    color: 'teal',
    '--MyColor': '#fff',
  })
  assert.deepEqual(splitCanvasCssDeclarations(' ; \n ; '), [])
  for (const invalid of ['font-family: "unfinished', 'background: url(x)', 'color:red; invalid'])
    assert.throws(() => splitCanvasCssDeclarations(invalid), { code: 'invalid_css' })
})

test('CSS resources, script expressions, selector rules and escape tricks cannot reach inline styles', () => {
  for (const css of [
    'background: url(https://example.com/x)',
    'background: URL("https://example.com/x")',
    'background: var(--image, url(https://example.com/x))',
    'background: image-set("https://example.com/a.png" 1x)',
    'background: -webkit-image-set("x" 1x)',
    'width: expression(alert(1))',
    'behavior: url(script.htc)',
    '-moz-binding: url(x)',
    '@import "x";',
    'color:red; body { display:none }',
    'background: u/**/rl(x)',
    'background: u\\72l(x)',
    'background: \\75rl(x)',
    '--remote: url(x); background: var(--remote)',
    'color: red !important',
    'font-family: "javascript:alert(1)"',
    'color: red\u0000',
    'background: paint(remote)',
    'width: attr(data-width px)',
    'content: "fake message"',
    'all: unset',
    '__proto__: value',
  ])
    assert.throws(() => parseCanvasCss(css), { code: 'invalid_css' }, css)
})

test('position is scoped to normal flow, relative or sticky without indirect overrides', () => {
  for (const position of ['static', 'relative', 'sticky'])
    assert.equal(parseCanvasCss(`position: ${position}`).position, position)
  for (const position of ['absolute', 'fixed', 'var(--position)', 'inherit', 'revert', 'initial'])
    assert.throws(() => parseCanvasCss(`position: ${position}`), { code: 'invalid_css' })
})

test('CSS length, delimiter structure and property declarations have bounded validation', () => {
  assert.equal(parseCanvasCss(`color:red;${' '.repeat(CANVAS_CSS_MAX_LENGTH - 10)}`).color, 'red')
  for (const css of [
    ' '.repeat(CANVAS_CSS_MAX_LENGTH + 1),
    'color',
    ':red',
    'color:',
    'color:red:blue',
    'width:calc(1px',
    'width:calc(1px))',
    'color:"red',
    'color:)',
    'color:[red)',
    'width:calc(1px;2px)',
    `width:${'calc('.repeat(17)}1px${')'.repeat(17)}`,
  ])
    assert.throws(() => parseCanvasCss(css), { code: 'invalid_css' })
})
