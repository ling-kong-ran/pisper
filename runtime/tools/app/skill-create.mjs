// 技能创建工具：校验并原子化创建标准 SKILL.md。
import { Type } from 'typebox'
import { defineTool } from '../../runtime/pi-coding-agent.mjs'

export const manifest = {
  id: 'skill_create',
  name: 'Skill Create',
  category: 'skills',
  risk: 'high',
  description:
    'Create a standards-compliant Agent Skill in the current project or global Pisper skills directory.',
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
    description: [
      manifest.description,
      `Project Skill path: ${cwd || '<current workspace>'}/.pisper/skills/<name>/SKILL.md.`,
      `Global Skill path: ${skillsRuntime?.skillsDir || '<PISPER_AGENT_DIR>/skills'}/<name>/SKILL.md.`,
      'The tool is registered and should be called directly for reusable Skill creation.',
    ].join('\n'),
    promptSnippet:
      'Create a reusable Agent Skill with a valid SKILL.md in the project or global skills directory',
    promptGuidelines: [
      'Use skill_create when the user asks to create, package, or persist a reusable Skill. This is the registered tool for the operation; do not replace it with manual file writes.',
      'Use project scope by default: it creates <current workspace>/.pisper/skills/<name>/SKILL.md. Use global scope only when the user explicitly wants the Skill available across projects; it creates under the global Pisper skills directory shown above.',
      'name must be lowercase kebab-case: 1-64 characters using lowercase letters, numbers, and single hyphens.',
      'description must say what the Skill does and when Agent should load it. instructions must be the Markdown body only, without YAML frontmatter.',
      'The generated SKILL.md uses YAML frontmatter with name and description, followed by the Markdown instructions. Include setup, workflow, constraints, and relative references to bundled scripts or assets when relevant.',
      'The tool never overwrites an existing Skill or directory. If it reports a conflict, inspect the existing Skill and ask before modifying it with filesystem tools.',
    ],
    parameters: Type.Object({
      name: Type.String({
        minLength: 1,
        maxLength: 64,
        pattern: '^[a-z0-9]+(?:-[a-z0-9]+)*$',
        description: 'Skill name using lowercase letters, numbers, and single hyphens',
      }),
      description: Type.String({
        minLength: 1,
        maxLength: 1024,
        description: 'What the Skill does and when Agent should use it',
      }),
      instructions: Type.String({
        minLength: 1,
        maxLength: 100000,
        description: 'Complete Markdown body for SKILL.md, without YAML frontmatter',
      }),
      scope: Type.Optional(
        Type.Union([Type.Literal('project'), Type.Literal('global')], {
          description: 'Project-local by default; global is available across projects',
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
