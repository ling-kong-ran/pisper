import assert from 'node:assert/strict'
import test from 'node:test'
import { createSkillCreateTool, manifest } from '../tools/app/skill-create.mjs'
import { TOOL_CATALOG, TOOL_PRESETS, createAppTools } from '../tools/registry.mjs'

test('skill_create is a high-risk app tool in writable presets', () => {
  assert.equal(manifest.risk, 'high')
  assert.ok(TOOL_CATALOG.some((tool) => tool.id === 'skill_create'))
  assert.equal(TOOL_PRESETS['read-only'].includes('skill_create'), false)
  assert.equal(TOOL_PRESETS.workspace.includes('skill_create'), true)
  assert.equal(TOOL_PRESETS.full.includes('skill_create'), true)
  assert.equal(createAppTools({ enabledTools: ['skill_create'] })[0]?.name, 'skill_create')
})

test('skill_create delegates structured input and invalidates Skill resources after creation', async () => {
  const calls = []
  const tool = createSkillCreateTool({
    cwd: '/workspace',
    skillsRuntime: {
      async create(input, options) {
        calls.push({ input, options })
        return {
          name: input.name,
          description: input.description,
          scope: input.scope || 'project',
          filePath: '/workspace/.pisper/skills/release-helper/SKILL.md',
          command: '/skill:release-helper',
        }
      },
    },
    onSkillsChanged(result) {
      calls.push({ changed: result.name })
    },
  })

  const result = await tool.execute('skill-1', {
    name: 'release-helper',
    description: 'Creates release notes. Use for releases.',
    instructions: '# Release Helper\n\nInspect commits.',
  })

  assert.deepEqual(calls, [
    {
      input: {
        name: 'release-helper',
        description: 'Creates release notes. Use for releases.',
        instructions: '# Release Helper\n\nInspect commits.',
      },
      options: { cwd: '/workspace' },
    },
    { changed: 'release-helper' },
  ])
  assert.match(result.content[0].text, /Created project skill "release-helper"/)
  assert.equal(result.details.command, '/skill:release-helper')
})
