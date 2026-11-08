// 子代理定义：项目 <工作区>/.pisper/agents/*.md 与全局 <数据目录>/agents/*.md，同名时项目覆盖全局。
// 每个文件一份定义：YAML frontmatter 写名称、何时使用、模型、思考强度与工具范围，正文是给该子代理的固定说明。
// 模型优先写档位（fast / standard / strong），档位在设置里映射到具体模型，项目文件因此能跨机器、跨供应商共享；
// 也可以写 provider/modelId，用不了时退回 tier 指定的档位，再退回父会话模型。
import { opendir, readFile, stat } from 'node:fs/promises'
import { basename, extname, join, resolve } from 'node:path'
import { pisperError } from '../../shared/errors.mjs'
import { agentProfilesSkillMd } from '../prompts/agent-profiles.mjs'
import { parseFrontmatter } from '../runtime/pi-coding-agent.mjs'

export const SUBAGENT_MODEL_TIERS = Object.freeze(['fast', 'standard', 'strong'])
const THINKING_LEVELS = new Set(['off', 'minimal', 'low', 'medium', 'high', 'xhigh'])
// 档位也认中文写法（快 / 标准 / 强），用户手写的定义文件常这样写。
const TIER_ALIASES = [
  [/^(?:fast|快)$/, 'fast'],
  [/^(?:standard|标准)$/, 'standard'],
  [/^(?:strong|强)$/, 'strong'],
]
const NAME_PATTERN = /^[a-z0-9][a-z0-9_-]{0,47}$/
const MAX_PROFILES_PER_SCOPE = 50
const MAX_PROFILE_FILE_BYTES = 64 * 1024
const MAX_DESCRIPTION_CHARS = 500
const MAX_INSTRUCTIONS_CHARS = 12_000
const MAX_TOOLS = 64
const MAX_MODEL_REF_CHARS = 300

export function projectAgentProfilesDir(cwd) {
  return join(resolve(cwd), '.pisper', 'agents')
}

export function globalAgentProfilesDir(agentDir) {
  return join(resolve(agentDir), 'agents')
}

function normalizeTier(value) {
  const text = String(value ?? '')
    .trim()
    .toLowerCase()
  return TIER_ALIASES.find(([pattern]) => pattern.test(text))?.[1] || ''
}

function normalizeModelRef(value) {
  const ref = String(value ?? '').trim()
  const slash = ref.indexOf('/')
  return slash > 0 && slash < ref.length - 1 && ref.length <= MAX_MODEL_REF_CHARS ? ref : ''
}

// 派发时直接指定的档位（spawn_agent 的 tier、工作流 agent() 的 options.tier）：认中英文写法，其余为空。
export function normalizeSubagentTier(value) {
  return normalizeTier(value)
}

// 设置里保存的档位映射：只认三个档位，值是 "provider/modelId"，非法项丢弃。
export function normalizeSubagentModelTiers(value) {
  const tiers = {}
  if (!value || typeof value !== 'object') return tiers
  for (const tier of SUBAGENT_MODEL_TIERS) {
    const ref = normalizeModelRef(value[tier])
    if (ref) tiers[tier] = ref
  }
  return tiers
}

function toolList(value) {
  const items = Array.isArray(value)
    ? value
    : typeof value === 'string'
      ? value.split(/[\s,]+/)
      : []
  const tools = []
  for (const item of items) {
    const name = String(item ?? '').trim()
    if (/^[A-Za-z0-9_.-]{1,64}$/.test(name) && !tools.includes(name)) tools.push(name)
    if (tools.length >= MAX_TOOLS) break
  }
  return tools
}

/**
 * 解析一份定义文件。不可信输入：字段逐个校验，出错时抛出可直接展示的原因。
 * @param {string} content
 * @param {{ fileName: string, scope: 'project' | 'global' }} source
 */
export function parseAgentProfile(content, { fileName, scope }) {
  let parsed
  try {
    parsed = parseFrontmatter(String(content || ''))
  } catch (error) {
    throw pisperError('invalid', 'agent', `frontmatter is not valid YAML: ${error.message}`)
  }
  const meta =
    parsed.frontmatter && typeof parsed.frontmatter === 'object' ? parsed.frontmatter : {}
  const name = String(meta.name ?? basename(fileName, extname(fileName)))
    .trim()
    .toLowerCase()
  if (!NAME_PATTERN.test(name))
    throw pisperError('invalid', 'agent', 'name: lowercase letters, digits, - and _, max 48 chars')
  const description = String(meta.description ?? '').trim()
  if (!description) throw pisperError('invalid', 'agent', 'description is required')
  if (description.length > MAX_DESCRIPTION_CHARS)
    throw pisperError('invalid', 'agent', `description: max ${MAX_DESCRIPTION_CHARS} chars`)
  const instructions = String(parsed.body || '').trim()
  if (instructions.length > MAX_INSTRUCTIONS_CHARS)
    throw pisperError('invalid', 'agent', `instructions: max ${MAX_INSTRUCTIONS_CHARS} chars`)

  const modelValue = meta.model == null ? '' : String(meta.model).trim()
  let tier = normalizeTier(modelValue)
  let modelRef = ''
  if (modelValue && !tier) {
    modelRef = normalizeModelRef(modelValue)
    if (!modelRef)
      throw pisperError(
        'invalid',
        'agent',
        'model: one of fast, standard, strong, or provider/modelId',
      )
  }
  if (meta.tier != null) {
    const fallbackTier = normalizeTier(meta.tier)
    if (!fallbackTier) throw pisperError('invalid', 'agent', 'tier: one of fast, standard, strong')
    // model 已经是档位时 tier 没有意义，以 model 为准。
    if (!tier) tier = fallbackTier
  }
  let thinkingLevel = ''
  if (meta.thinking != null) {
    thinkingLevel = String(meta.thinking).trim().toLowerCase()
    if (!THINKING_LEVELS.has(thinkingLevel))
      throw pisperError(
        'invalid',
        'agent',
        'thinking: one of off, minimal, low, medium, high, xhigh',
      )
  }
  const tools = meta.tools == null ? null : toolList(meta.tools)
  if (tools && !tools.length)
    throw pisperError('invalid', 'agent', 'tools must not be empty; omit it to keep all tools')
  return {
    name,
    description,
    modelRef,
    tier,
    thinkingLevel,
    tools,
    instructions,
    scope,
    file: scope === 'project' ? `.pisper/agents/${fileName}` : `agents/${fileName}`,
  }
}

async function readScope(dir, scope) {
  const profiles = []
  const errors = []
  let handle
  try {
    handle = await opendir(dir)
  } catch (error) {
    if (error?.code !== 'ENOENT' && error?.code !== 'ENOTDIR')
      errors.push({
        scope,
        file: '',
        message: pisperError('failed', 'folder', error.code || error.message).message,
      })
    return { profiles, errors }
  }
  const names = []
  for await (const entry of handle) {
    if (entry.isFile() && entry.name.toLowerCase().endsWith('.md')) names.push(entry.name)
  }
  names.sort((left, right) => left.localeCompare(right))
  for (const fileName of names) {
    const file = scope === 'project' ? `.pisper/agents/${fileName}` : `agents/${fileName}`
    if (profiles.length >= MAX_PROFILES_PER_SCOPE) {
      errors.push({
        scope,
        file,
        message: pisperError('limit', 'agent', `max ${MAX_PROFILES_PER_SCOPE} per folder`).message,
      })
      continue
    }
    try {
      const path = join(dir, fileName)
      if ((await stat(path)).size > MAX_PROFILE_FILE_BYTES)
        throw pisperError('too_large', 'file', `max ${MAX_PROFILE_FILE_BYTES / 1024} KB`)
      const profile = parseAgentProfile(await readFile(path, 'utf8'), { fileName, scope })
      if (profiles.some((item) => item.name === profile.name))
        throw pisperError('exists', 'agent', profile.name)
      profiles.push(profile)
    } catch (error) {
      errors.push({ scope, file, message: error.message })
    }
  }
  return { profiles, errors }
}

// 读取全部定义：项目覆盖全局同名定义；坏文件不影响其它定义，原因放在 errors 里给设置页展示。
export async function loadAgentProfiles({ agentDir, cwd }) {
  const [global, project] = await Promise.all([
    agentDir ? readScope(globalAgentProfilesDir(agentDir), 'global') : { profiles: [], errors: [] },
    cwd ? readScope(projectAgentProfilesDir(cwd), 'project') : { profiles: [], errors: [] },
  ])
  const byName = new Map()
  for (const profile of global.profiles) byName.set(profile.name, profile)
  for (const profile of project.profiles) {
    const overridden = byName.get(profile.name)
    byName.set(profile.name, overridden ? { ...profile, overrides: overridden.file } : profile)
  }
  return {
    profiles: [...byName.values()].sort((left, right) => left.name.localeCompare(right.name)),
    errors: [...project.errors, ...global.errors],
  }
}

// 写进 spawn_agent 工具说明的一行摘要，供父模型按任务挑选子代理：
// 写明会话打开时档位实际对应的模型、思考强度和工具范围，免得把要改文件的活派给只读定义。
// selection 是 resolveSubagentModel 的结果；不传时只写定义里声明的模型。
export function agentProfileSummary(profile, selection = null) {
  const declared = profile.modelRef || profile.tier
  const runsOn = !selection
    ? ''
    : selection.model
      ? ` → ${selection.model.provider}/${selection.model.id}`
      : ' → this session model'
  const parts = [`model: ${declared ? `${declared}${runsOn}` : 'this session model'}`]
  if (profile.thinkingLevel) parts.push(`thinking: ${profile.thinkingLevel}`)
  parts.push(`tools: ${profile.tools ? profile.tools.join(', ') : 'same as this session'}`)
  return `- ${profile.name} (${parts.join('; ')}): ${profile.description.replace(/\s+/g, ' ')}`
}

// 内置技能 pisper-subagents：用户让代理新建或修改子代理定义时，模型据此写出能通过上面校验的文件。
// 正文在 runtime/prompts/agent-profiles.mjs；限制直接取自本文件的常量，规则一改技能文字跟着变。
export const AGENT_PROFILES_SKILL = Object.freeze({
  name: 'pisper-subagents',
  files: Object.freeze({
    'SKILL.md': agentProfilesSkillMd({
      maxProfiles: MAX_PROFILES_PER_SCOPE,
      maxFileKb: MAX_PROFILE_FILE_BYTES / 1024,
      maxDescription: MAX_DESCRIPTION_CHARS,
      maxInstructions: MAX_INSTRUCTIONS_CHARS,
      maxTools: MAX_TOOLS,
    }),
  }),
})
