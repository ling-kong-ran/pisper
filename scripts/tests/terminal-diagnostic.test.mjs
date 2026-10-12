import assert from 'node:assert/strict'
import test from 'node:test'
import { createTerminalDiagnosticSanitizer } from '../terminal-diagnostic.mjs'

test('terminal diagnostics redact the most specific owned path and retain relative context', () => {
  const homeRoot = '/home/Synthetic Person'
  const repositoryRoot = `${homeRoot}/projects/pisper[probe]`
  const outputRoot = `${repositoryRoot}/release/usability/run-1`
  const workspace = `${outputRoot}/workspace`
  const sanitize = createTerminalDiagnosticSanitizer({
    workspace,
    outputRoot,
    repositoryRoot,
    homeRoot,
  })
  const original = {
    calls: [{ cwd: `${workspace}/terminal-ui-123/a`, rows: 24 }],
    tabs: [{ title: workspace, text: `error opening '${outputRoot}/home/cache'` }],
    captureError: `at ${repositoryRoot}/scripts/smoke.mjs:22:3\n${homeRoot}/.cache/browser`,
    acknowledged: 0,
    closed: false,
    missing: undefined,
    extra: null,
  }
  assert.deepEqual(sanitize(original), {
    calls: [{ cwd: '<workspace>/terminal-ui-123/a', rows: 24 }],
    tabs: [{ title: '<workspace>', text: "error opening '<owned-output>/home/cache'" }],
    captureError: 'at <repository>/scripts/smoke.mjs:22:3\n<home>/.cache/browser',
    acknowledged: 0,
    closed: false,
    missing: undefined,
    extra: null,
  })
  assert.equal(original.calls[0].cwd, `${workspace}/terminal-ui-123/a`)
})

test('Windows paths redact mixed separators, case variants, and native drive prefixes', () => {
  const sanitize = createTerminalDiagnosticSanitizer({
    workspace: 'C:\\Users\\Synthetic Person\\agent work\\pisper\\workspace',
    repositoryRoot: 'C:\\Users\\Synthetic Person\\agent work\\pisper',
    homeRoot: 'C:\\Users\\Synthetic Person',
  })
  assert.deepEqual(
    sanitize([
      'C:/Users/Synthetic Person/agent work/pisper/workspace/a',
      'c:\\USERS\\Synthetic Person/agent work\\pisper/workspace\\b',
      '\\\\?\\C:\\Users\\Synthetic Person\\agent work\\pisper\\workspace\\c',
      'at C:\\Users\\Synthetic Person\\agent work\\pisper\\scripts\\smoke.mjs:22:3',
      'C:\\Users\\Synthetic Person\\.cache\\browser',
    ]),
    [
      '<workspace>/a',
      '<workspace>\\b',
      '<workspace>\\c',
      'at <repository>\\scripts\\smoke.mjs:22:3',
      '<home>\\.cache\\browser',
    ],
  )
})

test('UNC roots redact both ordinary and native UNC spellings', () => {
  const sanitize = createTerminalDiagnosticSanitizer({
    workspace: '\\\\build-host\\share\\Synthetic Person\\workspace',
  })
  assert.deepEqual(
    sanitize([
      '\\\\build-host\\share\\Synthetic Person\\workspace\\a',
      '\\\\?\\UNC\\build-host\\share\\Synthetic Person\\workspace\\b',
      '//BUILD-HOST/share/Synthetic Person/workspace/c',
    ]),
    ['<workspace>\\a', '<workspace>\\b', '<workspace>/c'],
  )
})

test('console errors containing JSON-escaped Windows paths redact personal prefixes', () => {
  const sanitize = createTerminalDiagnosticSanitizer({
    workspace: 'C:\\Users\\Synthetic Person\\workspace',
  })
  assert.equal(
    sanitize(JSON.stringify({ cwd: 'C:\\Users\\Synthetic Person\\workspace\\a' })),
    JSON.stringify({ cwd: '<workspace>\\a' }),
  )
})

test('encoded file URLs and native-prefixed root configuration retain only relative paths', () => {
  const sanitize = createTerminalDiagnosticSanitizer({
    repositoryRoot: '\\\\?\\C:\\Users\\Synthetic Person\\project[probe]',
    homeRoot: '/home/Synthetic Person',
  })
  assert.deepEqual(
    sanitize([
      'at file:///C:/Users/Synthetic%20Person/project%5Bprobe%5D/scripts/smoke.mjs:22:3',
      'at file:///home/Synthetic%20Person/.cache/tool.mjs:1:2',
    ]),
    ['at file:///<repository>/scripts/smoke.mjs:22:3', 'at file://<home>/.cache/tool.mjs:1:2'],
  )
})

test('path matching respects directory boundaries and literal regex characters', () => {
  const sanitize = createTerminalDiagnosticSanitizer({ workspace: '/tmp/work[1]' })
  assert.equal(sanitize('/tmp/work[1]/a'), '<workspace>/a')
  assert.equal(sanitize('/tmp/work[1]-other/a /tmp/work1/a'), '/tmp/work[1]-other/a /tmp/work1/a')
})

test('path redaction preserves synthetic credential redaction', () => {
  const sanitize = createTerminalDiagnosticSanitizer({})
  assert.deepEqual(
    sanitize({
      text: 'rust_usability_sandbox_only synthetic-rust-test-key synthetic-mcp-query-secret',
      console: 'Bearer fixture-value __pisper_desktop=fixture-cookie; ?token=fixture-token&next=ok',
    }),
    {
      text: '[REDACTED] [REDACTED] [REDACTED]',
      console: 'Bearer [REDACTED] __pisper_desktop=[REDACTED]; ?token=[REDACTED]&next=ok',
    },
  )
})
