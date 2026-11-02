// 子代理定义：项目/全局定义文件的解析与覆盖、档位映射，以及派生时按定义选模型、收窄工具、注入说明。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import {
  AGENT_PROFILES_SKILL,
  agentProfileSummary,
  loadAgentProfiles,
  normalizeSubagentModelTiers,
  parseAgentProfile,
} from '../services/agent-profiles.mjs'

const project = { fileName: 'scout.md', scope: 'project' }

test('a definition file parses tier, thinking, tools and instructions', () => {
  const profile = parseAgentProfile(
    `---
name: Scout
description: 只读检索代码库，回答“在哪里”的问题
model: fast
thinking: low
tools: [read, grep, read]
---

只报告文件路径和行号。
`,
    project,
  )
  assert.deepEqual(profile, {
    name: 'scout',
    description: '只读检索代码库，回答“在哪里”的问题',
    modelRef: '',
    tier: 'fast',
    thinkingLevel: 'low',
    tools: ['read', 'grep'],
    instructions: '只报告文件路径和行号。',
    scope: 'project',
    file: '.pisper/agents/scout.md',
  })
  assert.equal(
    agentProfileSummary(profile),
    '- scout (model: fast; thinking: low; tools: read, grep): 只读检索代码库，回答“在哪里”的问题',
  )
  // 会话打开时解析出的实际模型一并写出；档位没配时说明会用当前会话模型。
  assert.match(
    agentProfileSummary(profile, { model: { provider: 'cheap', id: 'mini' }, source: 'tier' }),
    /^- scout \(model: fast → cheap\/mini; /,
  )
  assert.match(
    agentProfileSummary(profile, { model: null, source: 'parent' }),
    /^- scout \(model: fast → this session model; /,
  )
  assert.match(
    agentProfileSummary({ ...profile, tier: '', tools: null, thinkingLevel: '' }),
    /^- scout \(model: this session model; tools: same as this session\): /,
  )
})

test('an explicit model keeps its tier as a fallback and the name defaults to the file name', () => {
  const profile = parseAgentProfile(
    '---\ndescription: 审查\nmodel: openai/gpt-x\ntier: 强\n---\n',
    { fileName: 'reviewer.md', scope: 'global' },
  )
  assert.equal(profile.name, 'reviewer')
  assert.equal(profile.modelRef, 'openai/gpt-x')
  assert.equal(profile.tier, 'strong')
  assert.equal(profile.tools, null)
  assert.equal(profile.file, 'agents/reviewer.md')
})

test('invalid definitions explain what to fix', () => {
  const cases = [
    ['---\nname: Bad Name\ndescription: x\n---\n', /name/],
    ['---\nname: ok\n---\n', /description/],
    ['---\ndescription: x\nmodel: turbo\n---\n', /model/],
    ['---\ndescription: x\ntier: huge\n---\n', /tier/],
    ['---\ndescription: x\nthinking: max\n---\n', /thinking/],
    ['---\ndescription: x\ntools: []\n---\n', /tools/],
    ['---\ndescription: [unclosed\n---\n', /YAML/],
  ]
  for (const [content, pattern] of cases)
    assert.throws(() => parseAgentProfile(content, project), pattern)
})

test('tier settings keep only the three tiers with provider/model values', () => {
  assert.deepEqual(
    normalizeSubagentModelTiers({ fast: ' a/x ', strong: 'bad', standard: 3, extra: 'b/y' }),
    { fast: 'a/x' },
  )
  assert.deepEqual(normalizeSubagentModelTiers(null), {})
})

test('project definitions override global ones and broken files are reported without hiding others', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-agent-profiles-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const agentDir = join(root, 'data')
  const cwd = join(root, 'project')
  await mkdir(join(agentDir, 'agents'), { recursive: true })
  await mkdir(join(cwd, '.pisper', 'agents'), { recursive: true })
  await writeFile(
    join(agentDir, 'agents', 'scout.md'),
    '---\ndescription: 全局检索\nmodel: fast\n---\n',
  )
  await writeFile(
    join(agentDir, 'agents', 'writer.md'),
    '---\ndescription: 全局文档\nmodel: standard\n---\n',
  )
  await writeFile(
    join(cwd, '.pisper', 'agents', 'scout.md'),
    '---\ndescription: 项目检索\nmodel: strong\n---\n',
  )
  await writeFile(join(cwd, '.pisper', 'agents', 'broken.md'), '---\nname: broken\n---\n')
  await writeFile(join(cwd, '.pisper', 'agents', 'notes.txt'), 'ignored')

  const { profiles, errors } = await loadAgentProfiles({ agentDir, cwd })
  assert.deepEqual(
    profiles.map(({ name, scope, tier, overrides }) => ({ name, scope, tier, overrides })),
    [
      { name: 'scout', scope: 'project', tier: 'strong', overrides: 'agents/scout.md' },
      { name: 'writer', scope: 'global', tier: 'standard', overrides: undefined },
    ],
  )
  assert.equal(errors.length, 1)
  assert.equal(errors[0].file, '.pisper/agents/broken.md')
  assert.match(errors[0].message, /description/)
  // 返回给界面的只有相对位置，不暴露完整个人路径。
  assert.equal(JSON.stringify({ profiles, errors }).includes(root), false)
  assert.deepEqual(await loadAgentProfiles({ agentDir: join(root, 'none'), cwd: root }), {
    profiles: [],
    errors: [],
  })
})

function chunk(delta, finish_reason = null) {
  return {
    id: 'test',
    object: 'chat.completion.chunk',
    created: 0,
    model: 'fixture',
    choices: [{ index: 0, delta, finish_reason }],
  }
}

// 两个 Provider（主模型 parent、便宜模型 cheap）共用一个模拟服务，按路径区分并记下每次请求。
async function profileRuntime(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-agent-profile-runtime-'))
  const requests = []
  const server = createServer(async (request, reply) => {
    if (request.method === 'GET' && request.url.endsWith('/models')) {
      reply.writeHead(200, { 'content-type': 'application/json' })
      reply.end(JSON.stringify({ data: [{ id: 'fixture' }] }))
      return
    }
    const chunks = []
    for await (const part of request) chunks.push(part)
    const target = request.url.startsWith('/cheap') ? 'cheap' : 'parent'
    requests.push({ target, body: JSON.parse(Buffer.concat(chunks).toString('utf8')) })
    reply.writeHead(200, { 'content-type': 'text/event-stream' })
    reply.write(
      `data: ${JSON.stringify(chunk({ role: 'assistant', content: `${target} 完成` }))}\n\n`,
    )
    reply.write(
      `data: ${JSON.stringify({ ...chunk({}, 'stop'), usage: { prompt_tokens: 10, completion_tokens: 3, total_tokens: 13 } })}\n\n`,
    )
    reply.end('data: [DONE]\n\n')
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  let runtime
  t.after(async () => {
    await runtime?.dispose()
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
    await rm(directory, { recursive: true, force: true })
  })
  const base = `http://127.0.0.1:${server.address().port}`
  const provider = (path) => ({
    api: 'openai-completions',
    baseUrl: `${base}/${path}/v1`,
    models: [
      {
        id: 'fixture',
        name: 'Fixture',
        reasoning: false,
        input: ['text'],
        contextWindow: 200000,
        maxTokens: 8192,
      },
    ],
  })
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: { 'test-parent': provider('parent'), 'test-cheap': provider('cheap') },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({
      'test-parent': { type: 'api_key', key: 'test-only-parent-key' },
      'test-cheap': { type: 'api_key', key: 'test-only-cheap-key' },
    }),
  )
  await writeFile(join(directory, 'settings.json'), JSON.stringify({ retry: { enabled: false } }))
  await mkdir(join(directory, '.pisper', 'agents'), { recursive: true })
  await writeFile(
    join(directory, '.pisper', 'agents', 'scout.md'),
    '---\ndescription: 简单检索交给便宜模型\nmodel: fast\nthinking: low\ntools: [read, memory_search, web_search, ghost_tool]\n---\nSCOUT-STANDING-INSTRUCTIONS\n',
  )
  await writeFile(
    join(directory, '.pisper', 'agents', 'broken.md'),
    '---\ndescription: 思考强度写错了\nthinking: extreme\n---\n',
  )
  await writeFile(
    join(directory, '.pisper', 'agents', 'pinned.md'),
    '---\ndescription: 指定了一个没有配置的模型\nmodel: test-gone/fixture\n---\n',
  )
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.memorySummarizer = { summarize: async (entries) => entries.map(() => '') }
  await runtime.init()
  runtime.memory.setSemanticSummarizer(null)
  const created = await runtime.createSession('子代理定义', directory)
  await runtime.setSessionModel(created.id, 'test-parent', 'fixture')
  const value = await runtime.getOrCreateSession(created.id)
  assert.ok(value?.multiAgentRuntime, '会话应装配多 Agent 运行时')
  return { runtime, requests, sessionId: created.id, multiAgentRuntime: value.multiAgentRuntime }
}

async function finished(runtime, sessionId, id) {
  const deadline = Date.now() + 10_000
  while (true) {
    const agent = runtime.multiAgents.find(sessionId, id)
    if (['completed', 'failed', 'interrupted'].includes(agent?.status)) return agent
    assert.ok(Date.now() < deadline, `子代理应在期限内结束，当前状态 ${agent?.status}`)
    await runtime.multiAgents.wait(sessionId, 500, id).catch(() => {})
  }
}

test('a definition with a tier runs on the mapped model with its tools and instructions', async (t) => {
  const { runtime, requests, sessionId, multiAgentRuntime } = await profileRuntime(t)
  const value = await runtime.getOrCreateSession(sessionId)
  const config = await runtime.setSubagentModelTiers({ tiers: { fast: 'test-cheap/fixture' } })
  assert.deepEqual(config.subagentModelTiers, { fast: 'test-cheap/fixture' })
  const spawned = await multiAgentRuntime.spawn({
    taskName: 'lookup',
    message: '找到入口文件',
    agent: 'Scout',
  })
  assert.equal(spawned.agentProfile, 'scout')
  assert.equal(spawned.model, 'test-cheap/fixture')
  assert.equal(spawned.modelSource, 'tier')
  assert.equal(spawned.thinkingLevel, 'low')
  // 定义只能收窄：父会话的 bash/write/edit 不会带给子代理，只保留只读的计划查看这类协作工具。
  // memory_search、web_search 平时要先经 discover_tools 才进入父会话；定义点名要它们时直接给，
  // 不要求父会话先用过。会话处于默认的需审批模式，执行模式也不减少子代理的工具。
  assert.equal(runtime.getSessionExecutionMode(sessionId), 'approval-required')
  assert.equal(value.session.getActiveToolNames().includes('memory_search'), false)
  assert.deepEqual(spawned.availableTools, ['memory_search', 'read', 'web_search', 'get_plan'])
  const agent = await finished(runtime, sessionId, spawned.id)
  assert.equal(agent.status, 'completed', agent.error)
  const childRequest = requests.find((item) => item.target === 'cheap')
  assert.ok(childRequest, '子代理请求应发往档位映射的模型')
  assert.equal(
    requests.some((item) => item.target === 'parent'),
    false,
  )
  const system = JSON.stringify(childRequest.body.messages[0])
  assert.match(system, /Agent definition: scout/)
  assert.match(system, /SCOUT-STANDING-INSTRUCTIONS/)
  const durable = JSON.parse(await readFile(join(runtime.dataDir, 'pisper-agents.json'), 'utf8'))
  const record = (durable.agents || durable.records || []).find((item) => item.id === spawned.id)
  assert.equal(record?.agentProfile, 'scout')
  assert.equal(record?.profileInstructions, 'SCOUT-STANDING-INSTRUCTIONS')
  // 真正发给模型的工具也只有这些，不只是记录里写着。
  assert.deepEqual(childRequest.body.tools.map((tool) => tool.function?.name || tool.name).sort(), [
    'get_plan',
    'memory_search',
    'read',
    'web_search',
  ])
  assert.match(system, /Standing instructions from this agent definition apply to every task/)
  // 定义里写了但父会话没有的工具不会凭空出现，派生结果会点名告诉父模型。
  assert.deepEqual(spawned.skippedProfileTools, ['ghost_tool'])
})

test('unavailable definition models fall back to the parent model and say why', async (t) => {
  const { runtime, requests, sessionId, multiAgentRuntime } = await profileRuntime(t)
  const unmapped = await multiAgentRuntime.spawn({
    taskName: 'unmapped',
    message: 'x',
    agent: 'scout',
  })
  assert.equal(unmapped.model, 'test-parent/fixture')
  assert.equal(unmapped.modelSource, 'parent')
  assert.deepEqual(unmapped.modelFallbackReasons, ['tier-unset'])
  const pinned = await multiAgentRuntime.spawn({
    taskName: 'pinned',
    message: 'x',
    agent: 'pinned',
  })
  assert.equal(pinned.modelSource, 'parent')
  assert.deepEqual(pinned.modelFallbackReasons, ['model-unavailable'])
  // 没写 tools 的定义沿用父会话此刻直接可用的工具，不会顺带拿到还没发现的可选工具。
  assert.ok(pinned.availableTools.includes('bash'))
  assert.equal(pinned.availableTools.includes('memory_search'), false)
  for (const agent of [unmapped, pinned])
    assert.equal((await finished(runtime, sessionId, agent.id)).status, 'completed')
  assert.equal(
    requests.some((item) => item.target === 'cheap'),
    false,
  )
  await assert.rejects(
    multiAgentRuntime.spawn({ taskName: 'missing', message: 'x', agent: 'nobody' }),
    /Unknown agent definition: nobody\. Available: pinned, scout\./,
  )
  // 文件存在但没通过校验时，报错带上原因，代理能照着改。
  await assert.rejects(
    multiAgentRuntime.spawn({ taskName: 'broken', message: 'x', agent: 'broken' }),
    /\.pisper\/agents\/broken\.md is invalid: thinking 只能是/,
  )
  assert.equal(runtime.multiAgents.list(sessionId).length, 2)
})

test('tier settings are validated on save and definitions are listed per workspace', async (t) => {
  const { runtime } = await profileRuntime(t)
  await assert.rejects(runtime.setSubagentModelTiers({ tiers: { fast: 'test-gone/x' } }), /不存在/)
  await assert.rejects(runtime.setSubagentModelTiers({ tiers: { turbo: 'a/b' } }), /未知/)
  await assert.rejects(
    runtime.setSubagentModelTiers({ tiers: { fast: 'nope' } }),
    /provider\/modelId/,
  )
  await assert.rejects(runtime.setSubagentModelTiers({}), /格式无效/)
  assert.deepEqual((await runtime.setSubagentModelTiers({ tiers: {} })).subagentModelTiers, {})
  const listed = await runtime.listAgentProfiles({ cwd: runtime.cwd })
  assert.deepEqual(
    listed.profiles.map((profile) => profile.name),
    ['pinned', 'scout'],
  )
  await assert.rejects(runtime.listAgentProfiles({ cwd: join(runtime.cwd, 'missing') }), /工作目录/)
  // 设置页不带项目时只看全局定义。
  assert.deepEqual(await runtime.listAgentProfiles(), { profiles: [], errors: [] })
})

test('the built-in pisper-subagents skill teaches a format the parser accepts', () => {
  const skill = AGENT_PROFILES_SKILL.files['SKILL.md']
  assert.equal(AGENT_PROFILES_SKILL.name, 'pisper-subagents')
  assert.match(skill, /^---\nname: pisper-subagents\ndescription: .+子代理定义/)
  // 要让模型在用户提出时自动加载，不能像 pisper-component 那样只允许显式调用。
  assert.doesNotMatch(skill, /disable-model-invocation/)
  const example = skill.match(/~~~markdown\n([\s\S]*?)~~~/)?.[1]
  assert.ok(example, '技能里应有完整的定义示例')
  const profile = parseAgentProfile(example, { fileName: 'scout.md', scope: 'project' })
  assert.equal(profile.tier, 'fast')
  assert.deepEqual(profile.tools, ['read', 'grep', 'find', 'ls'])
  for (const text of ['fast', 'standard', 'strong', 'provider/modelId', 'xhigh', '500', '12000'])
    assert.ok(skill.includes(text), `技能说明应提到 ${text}`)
})

test('the parent session sees the skill and each definition with its tier model and tools', async (t) => {
  const { runtime } = await profileRuntime(t)
  await runtime.setSubagentModelTiers({ tiers: { fast: 'test-cheap/fixture' } })
  // 工具说明在会话创建时生成，所以先配档位再开新会话。
  const created = await runtime.createSession('看定义', runtime.cwd)
  await runtime.setSessionModel(created.id, 'test-parent', 'fixture')
  const { session } = await runtime.getOrCreateSession(created.id)
  assert.match(session.systemPrompt, /pisper-subagents/)
  assert.match(session.systemPrompt, /新建或修改 Pisper 子代理定义/)
  const spawn = session.getToolDefinition('spawn_agent')
  assert.match(
    spawn.description,
    /- scout \(model: fast → test-cheap\/fixture; thinking: low; tools: read, memory_search, web_search, ghost_tool\): 简单检索/,
  )
  assert.match(spawn.description, /- pinned \(model: test-gone\/fixture → this session model; /)
  assert.doesNotMatch(spawn.description, /broken/)
})
