// skill_create 的说明。

export const SKILL_CREATE = Object.freeze({
  description:
    'Create a standards-compliant Agent Skill in the current project or global Pisper skills directory.',
  snippet:
    'Create a reusable Agent Skill with a valid SKILL.md in the project or global skills directory',
  guidelines: [
    'Use skill_create when the user asks to create, package, or persist a reusable Skill. This is the registered tool for the operation; do not replace it with manual file writes.',
    'Use project scope by default: it creates <current workspace>/.pisper/skills/<name>/SKILL.md. Use global scope only when the user explicitly wants the Skill available across projects; it creates under the global Pisper skills directory shown above.',
    'name must be lowercase kebab-case: 1-64 characters using lowercase letters, numbers, and single hyphens.',
    'description must say what the Skill does and when Agent should load it. instructions must be the Markdown body only, without YAML frontmatter.',
    'The generated SKILL.md uses YAML frontmatter with name and description, followed by the Markdown instructions. Include setup, workflow, constraints, and relative references to bundled scripts or assets when relevant.',
    'The tool never overwrites an existing Skill or directory. If it reports a conflict, inspect the existing Skill and ask before modifying it with filesystem tools.',
  ],
  params: {
    name: 'Skill name using lowercase letters, numbers, and single hyphens',
    description: 'What the Skill does and when Agent should use it',
    instructions: 'Complete Markdown body for SKILL.md, without YAML frontmatter',
    scope: 'Project-local by default; global is available across projects',
  },
})

// 工具说明里写出两个实际路径，模型不用再猜技能放在哪。
/** @param {{ cwd?: string, skillsDir?: string }} [paths] */
export function skillCreateDescription({ cwd, skillsDir } = {}) {
  return [
    SKILL_CREATE.description,
    `Project Skill path: ${cwd || '<current workspace>'}/.pisper/skills/<name>/SKILL.md.`,
    `Global Skill path: ${skillsDir || '<PISPER_AGENT_DIR>/skills'}/<name>/SKILL.md.`,
  ].join('\n')
}
