import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { resolve } from 'node:path'
import test from 'node:test'
import { formatSkillsForPrompt, loadSkills } from '@earendil-works/pi-coding-agent'

const execFileAsync = promisify(execFile)
const skillPath = resolve('.pisper/skills/prompt-cache-optimizer/SKILL.md')
const scriptPath = resolve(
  '.pisper/skills/prompt-cache-optimizer/scripts/measure-tool-overhead.mjs',
)

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

test('prompt cache optimizer measurement script verifies stable prompt and fixed gateway schemas', async () => {
  const { stdout } = await execFileAsync(process.execPath, [scriptPath], {
    cwd: process.cwd(),
    timeout: 30_000,
    maxBuffer: 2 * 1024 * 1024,
  })
  const result = JSON.parse(stdout)
  assert.equal(result.estimator, 'ceil(characters / 4)')
  assert.equal(result.defaultSystemPromptContainsSkill, false)
  // Compare tool schemas from the same runtime so workspace path length and an evolving
  // system prompt cannot distort the hot/cold optimization budget.
  assert.ok(result.currentConfigurationReference.fixedTokensSaved > 0)
  assert.ok(
    result.currentConfigurationReference.schemaTokenReductionPercent >= 50,
    `schemaTokenReductionPercent=${result.currentConfigurationReference.schemaTokenReductionPercent}`,
  )
  assert.equal(result.hot.promptMatchesHot, true)
  for (const scenario of result.scenarios) {
    assert.equal(
      scenario.promptMatchesHot,
      true,
      `${scenario.label} changed the stable system prompt`,
    )
    assert.equal(
      scenario.hotSchemaIsExactPrefix,
      true,
      `${scenario.label} changed the fixed tool schemas`,
    )
    assert.equal(
      scenario.toolSchemaTokens,
      result.hot.toolSchemaTokens,
      `${scenario.label} changed the fixed tool schemas`,
    )
  }
})
