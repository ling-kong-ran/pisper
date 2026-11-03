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
// 限制直接取自本文件的常量，规则一改技能文字跟着变；不设 disable-model-invocation，模型按需自动加载。
const AGENT_PROFILES_SKILL_MD = `---
name: pisper-subagents
description: 新建或修改 Pisper 子代理定义（.pisper/agents/*.md）：给不同任务指定模型档位、思考强度、工具范围和固定说明。用户要求定义、新增、调整子代理或给子代理换模型时先读这份说明。
---

# Pisper 子代理定义

子代理定义是一个 Markdown 文件：frontmatter 写名称、何时使用、模型、思考强度和工具范围，正文是派给这个子代理的固定说明。
父会话调用 \`spawn_agent\` 时传 \`agent: <name>\`，子代理就按定义选模型、收窄工具，并把正文追加到自己的系统提示里。

## 放在哪里

- **项目定义**：\`<项目根目录>/.pisper/agents/<name>.md\`，可以随仓库提交，只在这个项目的会话里生效。用户没特别说明时写这里。
- **全局定义**：数据目录下的 \`agents/<name>.md\`，对所有项目生效；本技能文件位于 \`<数据目录>/builtin-skills/pisper-subagents/\`，全局目录就是往上两级的 \`agents/\`。只有用户明确要“所有项目都能用”时才写这里。
- 同名时项目定义覆盖全局定义。每处最多 ${MAX_PROFILES_PER_SCOPE} 个定义，单个文件不超过 ${MAX_PROFILE_FILE_BYTES / 1024} KB；目录里只读 \`.md\` 文件，不读子目录。

## 文件格式

~~~markdown
---
name: scout
description: 只读检索代码库，回答“某个东西在哪里、怎么用”的问题；不改文件。
model: fast
thinking: low
tools: [read, grep, find, ls]
---

只报告文件路径与行号，附最短的相关片段；找不到就说找不到，不要猜。
~~~

| 字段 | 必填 | 规则 |
| --- | --- | --- |
| \`name\` | 否 | 小写字母、数字、\`-\`、\`_\`，以字母或数字开头，最长 48 个字符；不写就用文件名。大写会被转成小写。 |
| \`description\` | 是 | 最长 ${MAX_DESCRIPTION_CHARS} 个字符。父模型只看这一行决定派不派给它：写清楚什么任务该交给它、它不做什么（例如“只读，不改文件”）。 |
| \`model\` | 否 | 档位 \`fast\` / \`standard\` / \`strong\`（也认 \`快\` / \`标准\` / \`强\`），或具体模型 \`provider/modelId\`。不写就用父会话的模型。 |
| \`tier\` | 否 | 只在 \`model\` 写了具体模型时有用：那个模型用不了时退回的档位。 |
| \`thinking\` | 否 | \`off\`、\`minimal\`、\`low\`、\`medium\`、\`high\`、\`xhigh\` 之一。不写就跟父会话一致。 |
| \`tools\` | 否 | 工具名列表（YAML 数组或逗号分隔），最多 ${MAX_TOOLS} 个。写了就不能为空；不限制工具时删掉这一项。 |

正文（frontmatter 之后）是固定说明，最长 ${MAX_INSTRUCTIONS_CHARS} 个字符，可以为空。每次派生时父会话还会另外写具体任务，正文只放这个子代理每次都要遵守的做法和输出格式。

## 选模型

- **优先写档位，不要写死具体模型。** 档位在“设置 › 模型 › 子代理”里映射到本机配置的模型，项目文件因此能在不同机器、不同供应商之间共享。
  - \`fast\`：检索、汇总、格式整理这类简单活；
  - \`standard\`：常规实现和排查；
  - \`strong\`：架构判断、疑难问题、代码审查。
- 只有用户点名要某个模型时才写 \`provider/modelId\`，并配一个 \`tier\` 作为退路。
- 只想按任务难度换模型时不必写定义：\`spawn_agent\` 直接传 \`tier\` 即可。定义适合需要固定说明、收窄工具或固定思考强度的场景；定义写了模型或档位时以定义为准。
- 退回顺序：具体模型 → \`tier\` 档位 → 派发时传的 \`tier\` → 父会话模型。用不了的原因（模型不可用、档位没设置、档位的模型不可用）会显示在子代理活动行和 \`spawn_agent\` 的返回里。
- 档位在设置里还没对应模型时，子代理会用父会话模型；写完定义后提醒用户去设置里给用到的档位选模型。

## 工具范围

- \`tools\` 只能**收窄**：子代理拿到的是父会话可用的工具与这个列表的交集，列表里写了父会话没有的工具也不会多出来。
  联网搜索、MCP 这类要先经 \`discover_tools\` 才能用的工具也可以直接列上，子代理派出时就有，不要求父会话先用过。
  写之前先看你能用哪些工具（含 \`discover_tools\` 能找到的），用它们的准确名字。
- 执行模式不减少工具：子代理拿到的工具在三种模式下一样，调用时和父会话一样按模式逐次确认。写了但父会话没有的工具会被跳过，并写在 \`spawn_agent\` 的返回里。
- 没写 \`tools\` 时，子代理沿用父会话此刻直接可用的工具。
- \`get_plan\` 和团队协作工具总会保留，不用写进列表。
- 只读型子代理不要列 \`write\`、\`edit\`、\`bash\` 这类能改动工作区的工具，并在 \`description\` 里写明“只读”，父模型才不会把要改文件的活派给它。
- 审批模式和项目目录范围始终跟父会话一致，定义不能放宽权限。

## 写完之后

- 定义每次派生时重新读取，改完立即生效，不用重启。
- 当前会话的 \`spawn_agent\` 工具说明是会话打开时生成的，看不到刚写的定义；在这个会话里用它要直接传 \`agent: <name>\`，新开的会话会自动列出。
- 文件写错时，按名字派生会返回具体原因；全局定义的错误还会列在“设置 › 模型 › 子代理”里。
- 子代理不能再派生子代理。

## 自检

- [ ] \`description\` 说清了何时用、不做什么；
- [ ] \`model\` 用档位，除非用户点名具体模型；
- [ ] \`tools\` 里的名字都是当前存在的工具，且与 \`description\` 的承诺一致；
- [ ] 正文只写固定做法，不写某一次任务的细节；
- [ ] 告诉用户文件位置、它会在什么时候被用上，以及需要去设置里配哪个档位。
`

export const AGENT_PROFILES_SKILL = Object.freeze({
  name: 'pisper-subagents',
  files: Object.freeze({ 'SKILL.md': AGENT_PROFILES_SKILL_MD }),
})
