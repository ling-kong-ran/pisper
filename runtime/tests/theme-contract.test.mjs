import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import test from 'node:test'
import { compile } from '@tailwindcss/node'
import postcss from 'postcss'
import { customAccentStyleRules } from '../../src/lib/custom-accent.ts'
import { DEFAULT_UI_PREFERENCES } from '../../src/stores/ui-store.ts'

const css = await readFile(new URL('../../src/index.css', import.meta.url), 'utf8')

// 只计算主题根节点的层叠；实际浏览器布局另由工作台冒烟测试验证。
// attributes 描述根元素上其余 data-* 属性（如 { 'phone-shell': true }），未列出的视为不存在。
function theme(theme, accent, extra = '', attributes = {}) {
  const present = { theme, accent, ...attributes }
  const rules = []
  postcss.parse(css + extra).walkRules((rule) => {
    for (const selector of rule.selectors) {
      if (!/^:root(?:\[data-[\w-]+(?:='[^']+')?\])*$/.test(selector)) continue
      const conditions = [...selector.matchAll(/\[data-([\w-]+)(?:='([^']+)')?\]/g)]
      const matches = conditions.every(([, key, value]) =>
        value === undefined ? Boolean(present[key]) : present[key] === value,
      )
      if (matches) rules.push({ rule, specificity: conditions.length })
    }
  })
  const values = new Map()
  for (const { rule } of rules.sort((a, b) => a.specificity - b.specificity)) {
    rule.walkDecls((decl) => values.set(decl.prop, decl.value))
  }
  const resolve = (key, seen = new Set()) => {
    assert.ok(!seen.has(key), `circular theme variable ${key}`)
    seen.add(key)
    const value = values.get(key)
    assert.ok(value, `undefined theme variable ${key}`)
    return value.replace(/var\((--[\w-]+)\)/g, (_, ref) => resolve(ref, new Set(seen)))
  }
  return resolve
}

// 解析 #rgb/#rrggbb 与 rgb()/rgba()；半透明色按 WCAG 做法先合成到下层不透明底色上再计算。
function channels(color, under = '#ffffff') {
  const hex = color.match(/^#([\da-f]{3}|[\da-f]{6})$/i)
  if (hex) {
    const value = hex[1].length === 3 ? [...hex[1]].map((x) => x + x).join('') : hex[1]
    return [0, 2, 4].map((i) => parseInt(value.slice(i, i + 2), 16))
  }
  const rgb = color.match(/^rgba?\(([^)]+)\)$/i)
  assert.ok(rgb, `unsupported color ${color}`)
  const parts = rgb[1]
    .split(/[\s,/]+/)
    .filter(Boolean)
    .map(Number)
  const alpha = parts[3] ?? 1
  const base = channels(under)
  return parts.slice(0, 3).map((value, i) => value * alpha + base[i] * (1 - alpha))
}

function contrast(a, b, under = '#ffffff') {
  const luminance = (rgb) => {
    const linear = rgb.map((channel) => {
      const c = channel / 255
      return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
    })
    return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722
  }
  const background = channels(b, under)
  const backgroundHex = `#${background.map((c) => Math.round(c).toString(16).padStart(2, '0')).join('')}`
  const values = [luminance(channels(a, backgroundHex)), luminance(background)].sort(
    (x, y) => y - x,
  )
  return (values[0] + 0.05) / (values[1] + 0.05)
}

function hue(color) {
  const [r, g, b] = channels(color).map((c) => c / 255)
  const max = Math.max(r, g, b)
  const delta = max - Math.min(r, g, b)
  if (!delta) return null
  const h =
    max === r ? ((g - b) / delta) % 6 : max === g ? (b - r) / delta + 2 : (r - g) / delta + 4
  return (h * 60 + 360) % 360
}

const TEXT = 4.5
const BOUNDARY = 3
const STATUSES = ['run', 'attn', 'fail', 'done', 'queue']

// WCAG 配对表：规格 v2 第 6.2 节带对比度数值的组合（spec 为规格记录值，由下方测试从实际 token 复算），
// 外加侧栏、井、控件边框等同一批 token 的日常组合。[前景, 背景, 下限, 规格值, 半透明背景的下层底]
// v2 侧栏（--sidebar-bg）比纸面暗一级，文字与琥珀圆点必须在两种底上都达标。
const WCAG_PAIRS = {
  light: [
    ['--text-muted', '--bg', TEXT, 5.05],
    ['--text-muted', '--sidebar-bg', TEXT, 4.71],
    ['--text-muted', '--panel', TEXT],
    ['--text-muted', '--solid', TEXT],
    ['--text-muted', '--surface-subtle', TEXT],
    ['--text-muted', '--well-bg', TEXT],
    ['--text-muted', '--lane-bg', TEXT],
    ['--text-muted', '--surface-muted', TEXT],
    ['--text-secondary', '--surface-hover', TEXT],
    ['--text-secondary', '--surface-active', TEXT],
    ['--text', '--bg', TEXT],
    ['--text', '--surface-active', TEXT],
    ['--on-accent', '--star', TEXT],
    ['--on-accent', '--star-hover', TEXT],
    ['--success', '--panel', TEXT],
    ['--danger', '--panel', TEXT],
    ['--warning-strong', '--panel', TEXT, 6.33],
    ['--warning-strong', '--sidebar-bg', TEXT],
    ['--warning-strong', '--warning-soft', TEXT],
    ['--on-attn', '--st-attn-solid', TEXT],
    ['--well-text', '--well-bg', TEXT],
    ['--well-muted', '--well-bg', TEXT],
    ['--control-border', '--panel', BOUNDARY],
    ['--control-border', '--solid', BOUNDARY],
    ['--control-border', '--bg', BOUNDARY],
    ['--control-border', '--sidebar-bg', BOUNDARY],
    ['--control-border', '--surface-subtle', BOUNDARY],
    ['--amber', '--sidebar-bg', BOUNDARY, 3.31],
    ['--amber', '--panel', BOUNDARY],
    ['--amber', '--surface-hover', BOUNDARY],
    ['--st-attn-ring', '--panel', BOUNDARY],
    ['--focus', '--bg', BOUNDARY],
    ['--focus', '--panel', BOUNDARY],
    ['--focus', '--sidebar-bg', BOUNDARY],
    ['--focus', '--lane-bg', BOUNDARY],
  ],
  dark: [
    ['--text-muted', '--bg', TEXT],
    ['--text-muted', '--panel', TEXT, 5.47],
    ['--text-muted', '--sidebar-bg', TEXT],
    ['--text-muted', '--solid', TEXT],
    ['--text-muted', '--surface-subtle', TEXT],
    ['--text-muted', '--well-bg', TEXT],
    ['--text-muted', '--lane-bg', TEXT],
    ['--text-muted', '--surface-muted', TEXT],
    ['--text-secondary', '--surface-hover', TEXT],
    ['--text-secondary', '--surface-active', TEXT],
    ['--text', '--bg', TEXT],
    ['--text', '--surface-active', TEXT],
    ['--on-accent', '--star', TEXT],
    ['--on-accent', '--star-hover', TEXT],
    ['--success', '--panel', TEXT],
    ['--danger', '--panel', TEXT],
    ['--warning-strong', '--panel', TEXT],
    ['--warning-strong', '--sidebar-bg', TEXT],
    ['--warning-strong', '--warning-soft', TEXT, undefined, '--panel'],
    ['--on-attn', '--st-attn-solid', TEXT],
    ['--well-text', '--well-bg', TEXT],
    ['--well-muted', '--well-bg', TEXT],
    ['--control-border', '--panel', BOUNDARY],
    ['--control-border', '--solid', BOUNDARY],
    ['--control-border', '--bg', BOUNDARY],
    ['--control-border', '--sidebar-bg', BOUNDARY],
    ['--control-border', '--surface-subtle', BOUNDARY],
    ['--amber', '--sidebar-bg', BOUNDARY],
    ['--amber', '--panel', BOUNDARY],
    ['--amber', '--surface-hover', BOUNDARY],
    ['--st-attn-solid', '--panel', BOUNDARY],
    ['--focus', '--bg', BOUNDARY],
    ['--focus', '--panel', BOUNDARY],
    ['--focus', '--sidebar-bg', BOUNDARY],
    ['--focus', '--lane-bg', BOUNDARY],
  ],
}
for (const mode of ['light', 'dark']) {
  for (const status of STATUSES) {
    // 状态文字落在卡片和页面底上；状态标签是 -soft 浅底（暗色为半透明，合成到卡片 --solid 上）。
    WCAG_PAIRS[mode].push(
      [`--st-${status}`, '--solid', TEXT],
      [`--st-${status}`, '--bg', TEXT],
      [`--st-${status}`, `--st-${status}-soft`, TEXT, undefined, '--solid'],
      // -fill 是卡片状态边与形状，只需界面元素边界对比度。
      [`--st-${status}-fill`, '--solid', BOUNDARY],
    )
  }
}

test('used semantic utilities generate declarations from the production Tailwind entry', async () => {
  const result = await compile(css, { base: path.resolve('src'), onDependency() {} })
  const utilities = [
    ['bg-surface-subtle', 'background-color'],
    ['bg-surface-hover', 'background-color'],
    ['text-brand', 'color'],
    // 第 0 期新增 token 的工具类映射。
    ...STATUSES.flatMap((status) => [
      [`text-st-${status}`, 'color'],
      [`bg-st-${status}-fill`, 'background-color'],
      [`bg-st-${status}-soft`, 'background-color'],
      [`border-st-${status}-border`, 'border-color'],
    ]),
    ['bg-st-attn-solid', 'background-color'],
    ['border-st-attn-ring', 'border-color'],
    ['text-on-attn', 'color'],
    ['bg-lane', 'background-color'],
    ['bg-well', 'background-color'],
    ['border-well-stroke', 'border-color'],
    ['text-well-text', 'color'],
    ['text-well-muted', 'color'],
    ['bg-diff-add', 'background-color'],
    ['bg-diff-add-mark', 'background-color'],
    ['bg-diff-del', 'background-color'],
    ['bg-diff-del-mark', 'background-color'],
    ['text-code-type', 'color'],
    ['rounded-card', 'border-radius'],
    ['shadow-card', '--tw-shadow'],
    ['shadow-float', '--tw-shadow'],
    ['shadow-edge', '--tw-shadow'],
  ]
  const output = postcss.parse(result.build(utilities.map(([name]) => name)))
  for (const [name, property] of utilities) {
    const selector = `.${name}`
    let found = false
    output.walkRules(selector, (rule) =>
      rule.walkDecls(property, () => {
        found = true
      }),
    )
    assert.ok(found, `${selector} must generate ${property}`)
  }
})

for (const mode of ['light', 'dark']) {
  for (const accent of ['neutral', 'blue', 'teal', 'violet', 'coral']) {
    test(`${mode} ${accent} solid accents keep readable text in normal and hover states`, () => {
      const resolve = theme(mode, accent)
      for (const background of ['--star', '--star-hover']) {
        const ratio = contrast(resolve('--on-accent'), resolve(background))
        assert.ok(ratio >= 4.5, `${background}: ${ratio.toFixed(2)} < 4.5`)
      }
    })
  }
}

test('custom colors keep text and focus visible including extreme and middle luminance colors', () => {
  for (const color of ['#ffffff', '#000000', '#1677e8', '#888888', '#00ff00', '#ff0000']) {
    for (const mode of ['light', 'dark']) {
      const resolve = theme(mode, 'custom', customAccentStyleRules(color))
      for (const background of ['--star', '--star-hover']) {
        assert.ok(
          contrast(resolve('--on-accent'), resolve(background)) >= 4.5,
          `${color} ${mode} ${background}`,
        )
      }
      for (const surface of ['--bg', '--solid', '--surface-subtle']) {
        assert.ok(
          contrast(resolve('--focus'), resolve(surface)) >= 3,
          `${color} ${mode} focus on ${surface}`,
        )
      }
    }
  }
})

// 规格 6.3：强调色预设只覆写这些变量，状态族与琥珀永远不随强调色变化。
const ACCENT_OWNED =
  /^--(?:star(?:-[\w-]+)?|brand-blue(?:-[\w-]+)?|on-accent|accent-ring|accent-fg|focus(?:-ring)?)$/
const STATUS_TOKENS = [
  ...STATUSES.flatMap((status) =>
    ['', '-fill', '-soft', '-border'].map((suffix) => `--st-${status}${suffix}`),
  ),
  '--st-attn-solid',
  '--st-attn-ring',
  '--on-attn',
  '--hatch-attn',
  '--warning-soft',
  '--warning-strong',
  '--warning-border',
  '--amber',
]

test('accent preset blocks only override accent-owned variables', () => {
  const blocks = []
  postcss
    .parse(css + customAccentStyleRules('#ee22aa'))
    .walkRules(/data-accent=/, (rule) => blocks.push(rule))
  assert.ok(blocks.length >= 10, 'expected light and dark accent preset blocks')
  for (const rule of blocks) {
    rule.walkDecls((decl) => {
      assert.match(decl.prop, ACCENT_OWNED, `${rule.selector} must not set ${decl.prop}`)
    })
  }
})

test('user accent changes leave primary actions, hover surfaces and semantic statuses neutral or fixed', () => {
  for (const mode of ['light', 'dark']) {
    const baseline = theme(mode, 'neutral')
    for (const accent of ['blue', 'teal', 'violet', 'coral', 'custom']) {
      const resolve = theme(
        mode,
        accent,
        accent === 'custom' ? customAccentStyleRules('#ee22aa') : '',
      )
      for (const token of [
        '--primary',
        '--primary-foreground',
        '--accent',
        '--accent-foreground',
        '--status-blue',
        '--success',
        '--danger',
        ...STATUS_TOKENS,
      ]) {
        assert.equal(resolve(token), baseline(token), `${mode} ${accent} must not recolor ${token}`)
      }
      assert.ok(contrast(resolve('--primary'), resolve('--primary-foreground')) >= 4.5)
    }
  }
})

test('graphite is the default accent, its focus is the brand blue and never amber', () => {
  assert.equal(DEFAULT_UI_PREFERENCES.accent, 'neutral')
  const light = theme('light', 'neutral')
  assert.equal(light('--focus'), light('--brand-blue'))
  assert.equal(light('--star'), light('--primary'), 'graphite primary action is the ink color')
  for (const mode of ['light', 'dark']) {
    for (const accent of ['neutral', 'blue', 'teal', 'violet', 'coral']) {
      const resolve = theme(mode, accent)
      const focusHue = hue(resolve('--focus'))
      assert.ok(
        focusHue === null || focusHue < 20 || focusHue > 60,
        `${mode} ${accent} focus ${resolve('--focus')} must not be amber`,
      )
      for (const token of ['--st-attn', '--st-attn-fill', '--st-attn-solid', '--amber'])
        assert.notEqual(resolve('--focus'), resolve(token), `${mode} ${accent} focus is ${token}`)
    }
  }
})

for (const mode of ['light', 'dark']) {
  test(`${mode} WCAG pair table keeps text at 4.5:1 and UI boundaries at 3:1`, () => {
    const resolve = theme(mode, 'neutral')
    const failures = []
    for (const [foreground, background, minimum, , under] of WCAG_PAIRS[mode]) {
      const ratio = contrast(
        resolve(foreground),
        resolve(background),
        under ? resolve(under) : undefined,
      )
      if (ratio < minimum)
        failures.push(`${foreground} on ${background}: ${ratio.toFixed(2)} < ${minimum}`)
    }
    assert.deepEqual(failures, [])
  })
}

test('spec contrast values are reproduced from the actual tokens', () => {
  for (const mode of ['light', 'dark']) {
    const resolve = theme(mode, 'neutral')
    for (const [foreground, background, , spec] of WCAG_PAIRS[mode]) {
      if (spec === undefined) continue
      const ratio = contrast(resolve(foreground), resolve(background))
      assert.ok(
        Math.abs(ratio - spec) < 0.01,
        `${mode} ${foreground} on ${background}: ${ratio.toFixed(2)} drifted from spec ${spec}`,
      )
    }
  }
})

test('new layout tokens resolve in both themes and cards round up on the phone shell', () => {
  for (const mode of ['light', 'dark']) {
    const resolve = theme(mode, 'neutral')
    for (const token of [
      ...STATUS_TOKENS,
      '--lane-bg',
      '--well-bg',
      '--well-stroke',
      '--well-text',
      '--well-muted',
      '--diff-add-bg',
      '--diff-add-mark',
      '--diff-del-bg',
      '--diff-del-mark',
      '--code-type',
      '--sh-1',
      '--sh-float',
      '--edge-highlight',
      '--d-lane',
    ])
      assert.ok(resolve(token), `${mode} ${token}`)
    assert.match(resolve('--hatch-attn'), /^repeating-linear-gradient\(/)
  }
  // 规格 6.4：会话行、列表行、小卡片 10，弹层与对话框 14，按钮芯片 6。
  assert.equal(theme('light', 'neutral')('--r-card'), '10px')
  assert.equal(theme('light', 'neutral')('--r-sm'), '6px')
  assert.equal(theme('light', 'neutral')('--r-lg'), '14px')
  assert.equal(
    theme('light', 'neutral', '', { 'phone-shell': true })('--r-card'),
    'calc(10px + 4px)',
  )
  assert.equal(
    theme('light', 'neutral', '', { 'phone-shell': true, radius: 'sharp' })('--r-card'),
    'calc(7px + 4px)',
  )
  assert.equal(theme('light', 'neutral')('--d-lane'), '240ms')
})

test('message surfaces remain subdued while text stays readable in both themes', () => {
  for (const mode of ['light', 'dark']) {
    const resolve = theme(mode, 'neutral')
    assert.ok(contrast(resolve('--user-bubble-text'), resolve('--user-bubble-bg')) >= 4.5)
    assert.ok(
      contrast(resolve('--user-bubble-bg'), resolve('--bg')) < 3,
      `${mode} message background must not become an inverted high-contrast surface`,
    )
    assert.ok(contrast(resolve('--text'), resolve('--surface-subtle')) >= 4.5)
  }
})
