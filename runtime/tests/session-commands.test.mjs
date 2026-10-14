import assert from 'node:assert/strict'
import test from 'node:test'
import { projectSessionCommands } from '../runtime/session-commands.mjs'

test('session command projection exposes safe Prompt and Skill invocations without resource contents', () => {
  const result = projectSessionCommands({
    sessionId: 'session-1',
    prompts: [
      {
        name: 'review',
        description: 'Review\u0000 staged changes',
        argumentHint: '<path>',
        content: 'private prompt body',
        filePath: 'C:/secret/prompts/review.md',
        sourceInfo: { scope: 'project', origin: 'top-level' },
      },
      {
        name: 'bad name',
        description: 'Cannot be invoked by Pi.',
        sourceInfo: { scope: 'user', origin: 'top-level' },
      },
    ],
    skills: [
      {
        name: 'docs-search',
        description: 'Search docs.',
        filePath: 'C:/secret/skills/docs-search/SKILL.md',
        sourceInfo: { scope: 'user', origin: 'package' },
      },
    ],
    diagnostics: [{ type: 'collision', path: 'C:/secret/prompts/other.md' }],
  })

  assert.deepEqual(result, {
    sessionId: 'session-1',
    commands: [
      {
        name: 'review',
        invocation: '/review',
        description: 'Review staged changes',
        argumentHint: '<path>',
        source: 'prompt',
        scope: 'project',
      },
      {
        name: 'docs-search',
        invocation: '/skill:docs-search',
        description: 'Search docs.',
        argumentHint: '',
        source: 'skill',
        scope: 'package',
      },
    ],
    counts: { total: 2, prompts: 1, skills: 1, diagnostics: 1 },
  })
  assert.doesNotMatch(JSON.stringify(result), /private prompt body|C:\/secret/)
})
