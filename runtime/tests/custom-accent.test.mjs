import assert from 'node:assert/strict'
import test from 'node:test'
import { customAccentStyleRules } from '../../src/lib/custom-accent.ts'

function rules(css) {
  return new Map(
    [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map(([, selector, body]) => [
      selector.trim(),
      new Map(
        body.split(';').flatMap((entry) => {
          const separator = entry.indexOf(':')
          return separator < 0
            ? []
            : [[entry.slice(0, separator).trim(), entry.slice(separator + 1).trim()]]
        }),
      ),
    ]),
  )
}

const fixtures = [
  {
    color: '#1677e8',
    channels: [73, 149, 237],
    lightStrong: '#1662bb',
    darkStrong: '#81b6f3',
    lightOn: '#fff',
    darkOn: '#18181b',
  },
  {
    color: '#000000',
    channels: [56, 56, 56],
    lightStrong: '#050506',
    darkStrong: '#8a8a8a',
    lightOn: '#fff',
    darkOn: '#fff',
  },
  {
    color: '#ffffff',
    channels: [255, 255, 255],
    lightStrong: '#707072',
    darkStrong: '#ffffff',
    lightOn: '#18181b',
    darkOn: '#18181b',
  },
]

test('runtime custom accent preserves translucent dark surfaces without requiring color-mix support', () => {
  for (const fixture of fixtures) {
    const dark = rules(customAccentStyleRules(fixture.color)).get(
      ":root[data-theme='dark'][data-accent='custom']",
    )
    for (const [token, alpha] of [
      ['--star-soft', 0.15],
      ['--star-border', 0.38],
      ['--brand-blue-soft', 0.15],
      ['--brand-blue-border', 0.34],
    ]) {
      const match = /^rgba\((\d+), (\d+), (\d+), ([\d.]+)\)$/.exec(dark.get(token))
      assert.ok(match, `${token} must be a legacy-browser-compatible translucent color`)
      assert.deepEqual(match.slice(1, 4).map(Number), fixture.channels)
      assert.equal(Number(match[4]), alpha)
    }
  }
})

test('using compatible alpha syntax preserves readable foregrounds and solid custom colors in both themes', () => {
  for (const fixture of fixtures) {
    const styles = rules(customAccentStyleRules(fixture.color))
    const light = styles.get(":root[data-accent='custom']")
    const dark = styles.get(":root[data-theme='dark'][data-accent='custom']")
    assert.equal(light.get('--star'), fixture.color)
    assert.equal(light.get('--star-strong'), fixture.lightStrong)
    assert.equal(dark.get('--star-strong'), fixture.darkStrong)
    assert.equal(light.get('--on-accent'), fixture.lightOn)
    assert.equal(dark.get('--on-accent'), fixture.darkOn)
  }
})
