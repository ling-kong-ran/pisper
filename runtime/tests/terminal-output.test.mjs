import assert from 'node:assert/strict'
import test from 'node:test'
import {
  MAX_TERMINAL_DISPLAY_CHARS,
  TERMINAL_TRUNCATION_MARKER,
  stripTerminalControlSequences,
  terminalDisplayOutput,
} from '../../src/lib/terminal-output.ts'

test('terminal display keeps full short output while removing controls from streaming text', () => {
  const source = '\u001b[32mgreen\u001b[0m\n\u001b]0;ignored title\u0007plain\u0000text'
  const display = terminalDisplayOutput(source)
  assert.equal(display.truncated, false)
  assert.equal(display.text, 'green\nplaintext')
  assert.equal(stripTerminalControlSequences(source), display.text)
})

test('terminal display bounds expensive rendering while preserving the newest output', () => {
  const latest = 'LATEST-LINE'
  const source = `${'old output line\n'.repeat(2_000)}${latest}`
  const display = terminalDisplayOutput(source)
  assert.equal(display.truncated, true)
  assert.ok(display.text.startsWith(`${TERMINAL_TRUNCATION_MARKER}\n`))
  assert.ok(display.text.endsWith(latest))
  assert.ok(
    display.text.length <= MAX_TERMINAL_DISPLAY_CHARS + TERMINAL_TRUNCATION_MARKER.length + 1,
  )
})
