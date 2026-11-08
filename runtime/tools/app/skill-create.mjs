// 技能创建工具：校验并原子化创建标准 SKILL.md。
import { Type } from 'typebox'
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { SKILL_CREATE, skillCreateDescription } from '../../prompts/tools/skill-create.mjs'

export const manifest = {
  id: 'skill_create',
  name: 'Skill Create',
  category: 'skills',
  risk: 'high',
  description: SKILL_CREATE.description,
  scope: 'Current project or global Pisper skills directory',
  capability: 'Validate and atomically create a new SKILL.md without overwriting existing content',
  source: 'app',
}

function resultContent(result) {
  return {
    content: [
      {
        type: 'text',
        text: [
          `Created ${result.scope} skill "${result.name}".`,
          `SKILL.md: ${result.filePath}`,
          `Directory: ${result.directory || ''}`,
          `Command: ${result.command || `/skill:${result.name}`}`,
          'Format: YAML frontmatter with name and description, followed by Markdown instructions.',
          'The skill will be available to Agent sessions on their next turn.',
        ].join('\n'),
      },
    ],
    details: result,
  }
}

export function createSkillCreateTool({ cwd, skillsRuntime, onSkillsChanged } = {}) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: skillCreateDescription({ cwd, skillsDir: skillsRuntime?.skillsDir }),
    promptSnippet: SKILL_CREATE.snippet,
    promptGuidelines: SKILL_CREATE.guidelines,
    parameters: Type.Object({
      name: Type.String({
        minLength: 1,
        maxLength: 64,
        pattern: '^[a-z0-9]+(?:-[a-z0-9]+)*$',
        description: SKILL_CREATE.params.name,
      }),
      description: Type.String({
        minLength: 1,
        maxLength: 1024,
        description: SKILL_CREATE.params.description,
      }),
      instructions: Type.String({
        minLength: 1,
        maxLength: 100000,
        description: SKILL_CREATE.params.instructions,
      }),
      scope: Type.Optional(
        Type.Union([Type.Literal('project'), Type.Literal('global')], {
          description: SKILL_CREATE.params.scope,
        }),
      ),
    }),
    async execute(_toolCallId, params) {
      if (!skillsRuntime?.create) throw new Error('Pisper skills runtime is not initialized.')
      const result = await skillsRuntime.create(params, { cwd })
      await onSkillsChanged?.(result)
      return resultContent(result)
    },
  })
}
