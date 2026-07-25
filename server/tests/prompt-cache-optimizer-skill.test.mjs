import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { resolve } from 'node:path'
import test from 'node:test'
import { formatSkillsForPrompt, loadSkills } from '@earendil-works/pi-coding-agent'

const execFileAsync = promisify(execFile)
const skillPath = resolve('.agents/skills/prompt-cache-optimizer/SKILL.md')
const scriptPath = resolve('.agents/skills/prompt-cache-optimizer/scripts/measure-tool-overhead.mjs')

test('prompt cache optimizer skill is hidden from default model context and remains explicitly invocable', () => {
  const loaded = loadSkills({
    cwd: process.cwd(),
    agentDir: process.cwd(),
    skillPaths: [skillPath],
    includeDefaults: false,
  })
  const skill = loaded.skills.find((item) => item.name === 'prompt-cache-optimizer')
  assert.ok(skill)
  assert.equal(skill.disableModelInvocation, true)
  assert.equal(formatSkillsForPrompt([skill]), '')
  assert.doesNotMatch(formatSkillsForPrompt(loaded.skills), /prompt-cache-optimizer/)
})

test('prompt cache optimizer measurement script verifies stable prompt and appended cold schemas', async () => {
  const { stdout } = await execFileAsync(process.execPath, [scriptPath], {
    cwd: process.cwd(),
    timeout: 30_000,
    maxBuffer: 2 * 1024 * 1024,
  })
  const result = JSON.parse(stdout)
  assert.equal(result.estimator, 'ceil(characters / 4)')
  assert.equal(result.defaultSystemPromptContainsSkill, false)
  // Absolute percent can move a few tenths across runners because the system prompt
  // embeds the working directory path length. Keep a clear win over the full-tool baseline.
  assert.ok(result.historicalReference.reductionPercent >= 45, `reductionPercent=${result.historicalReference.reductionPercent}`)
  assert.ok(result.hot.fixedTokens < result.historicalReference.fixedTokensBeforeHotColdOptimization * 0.55)
  assert.equal(result.hot.promptMatchesHot, true)
  for (const scenario of result.scenarios) {
    assert.equal(scenario.promptMatchesHot, true, `${scenario.label} changed the stable system prompt`)
    assert.equal(scenario.hotSchemaIsExactPrefix, true, `${scenario.label} did not append its cold schemas`)
  }
})
