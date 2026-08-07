import assert from 'node:assert/strict'
import test from 'node:test'
import {
  hasMeaningfulGeneratedNotes,
  normalizeReleaseNotes,
  releaseNotesMarkdown,
} from '../../shared/release-notes.mjs'

test('updater HTML release notes are converted to safe Markdown', () => {
  const html =
    '<h2>Pisper 0.1.1</h2><p><strong>Full Changelog</strong>: <a href="https://github.com/ling-kong-ran/pisper/commits/v0.1.1">https://github.com/ling-kong-ran/pisper/commits/v0.1.1</a></p>'
  assert.equal(
    normalizeReleaseNotes(html),
    [
      '## Pisper 0.1.1',
      '',
      '**Full Changelog**: [https://github.com/ling-kong-ran/pisper/commits/v0.1.1](https://github.com/ling-kong-ran/pisper/commits/v0.1.1)',
    ].join('\n'),
  )
})

test('release notes preserve Markdown and combine updater note arrays', () => {
  assert.equal(releaseNotesMarkdown('## Pisper\n\n- Fixed updates'), '## Pisper\n\n- Fixed updates')
  assert.equal(
    releaseNotesMarkdown([{ note: '<p>First fix</p>' }, { note: '<p>Second fix</p>' }]),
    'First fix\n\nSecond fix',
  )
})

test('generated release notes fall back when GitHub only returns a changelog link', () => {
  assert.equal(
    hasMeaningfulGeneratedNotes(
      '<!-- generated -->\n\n**Full Changelog**: https://example.com/commits/v1.0.0',
    ),
    false,
  )
  assert.equal(
    hasMeaningfulGeneratedNotes(
      "## What's Changed\n\n**Full Changelog**: https://example.com/compare/v1...v2",
    ),
    false,
  )
  assert.equal(
    hasMeaningfulGeneratedNotes(
      "## What's Changed\n\n- Fix updater rendering\n\n**Full Changelog**: https://example.com/compare/v1...v2",
    ),
    true,
  )
})
