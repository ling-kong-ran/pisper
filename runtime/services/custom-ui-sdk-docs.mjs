// 内置技能 pisper-component 的清单，供工作室绑定会话用 /skill:pisper-component 显式加载（计划 §5.3）。
// 正文（SKILL.md 与 pisper-component.d.ts）是给模型的文字，统一放在 runtime/prompts/custom-ui.mjs；
// 启动时由 SkillsService 按内容哈希写到 <dataDir>/builtin-skills/。
import { PISPER_COMPONENT_DTS, PISPER_COMPONENT_SKILL_MD } from '../prompts/custom-ui.mjs'

export { PISPER_COMPONENT_DTS, PISPER_COMPONENT_SKILL_MD }

/** 内置技能清单：SkillsService 启动时按内容哈希写到 <dataDir>/builtin-skills/<name>/。 */
export const BUILTIN_SKILLS = Object.freeze([
  Object.freeze({
    name: 'pisper-component',
    files: Object.freeze({
      'SKILL.md': PISPER_COMPONENT_SKILL_MD,
      'pisper-component.d.ts': PISPER_COMPONENT_DTS,
    }),
  }),
])
