// 子代理定义：项目 <工作区>/.pisper/agents/*.md 与全局 <数据目录>/agents/*.md，同名时项目覆盖全局。
// 每个文件一份定义：YAML frontmatter 写名称、何时使用、模型、思考强度与工具范围，正文是给该子代理的固定说明。
// 模型优先写档位（fast / standard / strong），档位在设置里映射到具体模型，项目文件因此能跨机器、跨供应商共享；
// 也可以写 provider/modelId，用不了时退回 tier 指定的档位，再退回父会话模型。
import { opendir, readFile, stat } from 'node:fs/promises'
import { basename, extname, join, resolve } from 'node:path'
import { parseFrontmatter } from '../runtime/pi-coding-agent.mjs'

export const SUBAGENT_MODEL_TIERS = Object.freeze(['fast', 'standard', 'strong'])
const THINKING_LEVELS = new Set(['off', 'minimal', 'low', 'medium', 'high', 'xhigh'])
const TIER_ALIASES = new Map([
  ['fast', 'fast'],
  ['快', 'fast'],
  ['standard', 'standard'],
  ['标准', 'standard'],
  ['strong', 'strong'],
  ['强', 'strong'],
])
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
  return (
    TIER_ALIASES.get(
      String(value ?? '')
        .trim()
        .toLowerCase(),
    ) || ''
  )
}

function normalizeModelRef(value) {
  const ref = String(value ?? '').trim()
  const slash = ref.indexOf('/')
  return slash > 0 && slash < ref.length - 1 && ref.length <= MAX_MODEL_REF_CHARS ? ref : ''
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
    throw new Error(`frontmatter 不是有效的 YAML：${error.message}`)
  }
  const meta =
    parsed.frontmatter && typeof parsed.frontmatter === 'object' ? parsed.frontmatter : {}
  const name = String(meta.name ?? basename(fileName, extname(fileName)))
    .trim()
    .toLowerCase()
  if (!NAME_PATTERN.test(name))
    throw new Error('name 只能用小写字母、数字、连字符和下划线，最长 48 个字符。')
  const description = String(meta.description ?? '').trim()
  if (!description) throw new Error('缺少 description：写清楚什么任务该交给这个子代理。')
  if (description.length > MAX_DESCRIPTION_CHARS)
    throw new Error(`description 最长 ${MAX_DESCRIPTION_CHARS} 个字符。`)
  const instructions = String(parsed.body || '').trim()
  if (instructions.length > MAX_INSTRUCTIONS_CHARS)
    throw new Error(`正文说明最长 ${MAX_INSTRUCTIONS_CHARS} 个字符。`)

  const modelValue = meta.model == null ? '' : String(meta.model).trim()
  let tier = normalizeTier(modelValue)
  let modelRef = ''
  if (modelValue && !tier) {
    modelRef = normalizeModelRef(modelValue)
    if (!modelRef)
      throw new Error(
        'model 只能是 fast、standard、strong 之一，或 provider/modelId 形式的具体模型。',
      )
  }
  if (meta.tier != null) {
    const fallbackTier = normalizeTier(meta.tier)
    if (!fallbackTier) throw new Error('tier 只能是 fast、standard、strong 之一。')
    // model 已经是档位时 tier 没有意义，以 model 为准。
    if (!tier) tier = fallbackTier
  }
  let thinkingLevel = ''
  if (meta.thinking != null) {
    thinkingLevel = String(meta.thinking).trim().toLowerCase()
    if (!THINKING_LEVELS.has(thinkingLevel))
      throw new Error('thinking 只能是 off、minimal、low、medium、high、xhigh 之一。')
  }
  const tools = meta.tools == null ? null : toolList(meta.tools)
  if (tools && !tools.length) throw new Error('tools 写了就不能为空；不限制工具时删掉这一项。')
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
      errors.push({ scope, file: '', message: `无法读取定义目录：${error.code || error.message}` })
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
      errors.push({ scope, file, message: `每处最多 ${MAX_PROFILES_PER_SCOPE} 个子代理定义。` })
      continue
    }
    try {
      const path = join(dir, fileName)
      if ((await stat(path)).size > MAX_PROFILE_FILE_BYTES)
        throw new Error('定义文件不能超过 64 KB。')
      const profile = parseAgentProfile(await readFile(path, 'utf8'), { fileName, scope })
      if (profiles.some((item) => item.name === profile.name))
        throw new Error(`与同目录里的另一个定义重名：${profile.name}。`)
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

// 写进 spawn_agent 工具说明的一行摘要，供父模型按任务挑选子代理。
export function agentProfileSummary(profile) {
  const model = profile.modelRef || profile.tier || 'inherit'
  return `- ${profile.name} (model: ${model}): ${profile.description.replace(/\s+/g, ' ')}`
}
