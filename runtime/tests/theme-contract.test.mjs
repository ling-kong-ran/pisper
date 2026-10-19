import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import test from 'node:test'
import { compile } from '@tailwindcss/node'
import postcss from 'postcss'
import { customAccentStyleRules } from '../../src/lib/custom-accent.ts'

const css = await readFile(new URL('../../src/index.css', import.meta.url), 'utf8')

// 只计算主题根节点的层叠；实际浏览器布局另由工作台冒烟测试验证。
function theme(theme, accent, extra = '') {
  const rules = []
  postcss.parse(css + extra).walkRules((rule) => {
    for (const selector of rule.selectors) {
      if (!/^:root(?:\[data-(?:theme|accent)='[^']+'\])*$/.test(selector)) continue
      const attributes = [...selector.matchAll(/\[data-(theme|accent)='([^']+)'\]/g)]
      if (attributes.some(([, key, value]) => ({ theme, accent })[key] !== value)) continue
      rules.push({ rule, specificity: attributes.length })
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

function contrast(a, b) {
  const luminance = (hex) => {
    assert.match(hex, /^#[\da-f]{3}(?:[\da-f]{3})?$/i)
    let value = hex.slice(1)
    if (value.length === 3) value = [...value].map((x) => x + x).join('')
    const channels = [0, 2, 4].map((i) => {
      const c = parseInt(value.slice(i, i + 2), 16) / 255
      return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
    })
    return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722
  }
  const values = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (values[0] + 0.05) / (values[1] + 0.05)
}

test('used semantic utilities generate declarations from the production Tailwind entry', async () => {
  const result = await compile(css, { base: path.resolve('src'), onDependency() {} })
  const output = postcss.parse(
    result.build(['bg-surface-subtle', 'bg-surface-hover', 'text-brand']),
  )
  for (const [selector, property] of [
    ['.bg-surface-subtle', 'background-color'],
    ['.bg-surface-hover', 'background-color'],
    ['.text-brand', 'color'],
  ]) {
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
        '--amber',
      ]) {
        assert.equal(resolve(token), baseline(token), `${mode} ${accent} must not recolor ${token}`)
      }
      assert.ok(contrast(resolve('--primary'), resolve('--primary-foreground')) >= 4.5)
    }
  }
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
