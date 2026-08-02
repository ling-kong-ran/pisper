import { createHash } from 'node:crypto'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { AgentRuntimeService } from '../../../../server/runtime/agent-runtime.mjs'
import { applyPisperSystemPrompt } from '../../../../server/prompts/pisper-system-prompt.mjs'

const scriptDirectory = dirname(fileURLToPath(import.meta.url))
const projectRoot = resolve(scriptDirectory, '../../../..')
const temporaryDataDirectory = await mkdtemp(join(tmpdir(), 'pisper-prompt-cache-measure-'))

function estimatedTokens(value) {
  return Math.ceil(String(value || '').length / 4)
}

function serializedSchemas(session) {
  return JSON.stringify((session.agent.state.tools || []).map((tool) => ({
    type: 'function',
    function: {
      name: tool.name,
      description: tool.description || '',
      parameters: tool.parameters || {},
    },
  })))
}

function snapshot(session, label) {
  const systemPrompt = session.agent.state.systemPrompt
  const schemas = serializedSchemas(session)
  return {
    label,
    activeTools: session.getActiveToolNames(),
    systemPromptHash: createHash('sha256').update(systemPrompt).digest('hex'),
    systemPromptChars: systemPrompt.length,
    systemPromptTokens: estimatedTokens(systemPrompt),
    toolSchemaChars: schemas.length,
    toolSchemaTokens: estimatedTokens(schemas),
    fixedTokens: estimatedTokens(systemPrompt) + estimatedTokens(schemas),
    systemPrompt,
    schemas,
  }
}

function publicSnapshot(value, hot) {
  return {
    label: value.label,
    activeTools: value.activeTools,
    systemPromptHash: value.systemPromptHash,
    systemPromptChars: value.systemPromptChars,
    systemPromptTokens: value.systemPromptTokens,
    toolSchemaChars: value.toolSchemaChars,
    toolSchemaTokens: value.toolSchemaTokens,
    fixedTokens: value.fixedTokens,
    promptMatchesHot: value.systemPrompt === hot.systemPrompt,
    hotSchemaIsExactPrefix: value.schemas.startsWith(hot.schemas.slice(0, -1)),
  }
}

let runtime
try {
  runtime = new AgentRuntimeService({ cwd: projectRoot, dataDir: temporaryDataDirectory })
  await runtime.init()
  const created = await runtime.createSession('Prompt cache measurement')
  const value = await runtime.getOrCreateSession(created.id)
  const { session } = value

  const hot = snapshot(session, 'hot')
  const scenarios = []
  for (const [label, message] of [
    ['web-search', '请搜索官网的最新版本说明'],
    ['browser', '打开 https://example.com 并截图'],
    ['visual', '生成一张产品海报'],
    ['memory', '记住我的默认语言是中文'],
    ['multi-agent', '派一个 Agent 并行审查测试'],
    ['mcp-management', '列出 MCP 服务'],
  ]) {
    await runtime.selectToolsForMessage(value, message)
    scenarios.push(snapshot(session, label))
  }

  session.setActiveToolsByName(session.getAllTools().map((tool) => tool.name))
  applyPisperSystemPrompt(session, session.model)
  const allConfigured = snapshot(session, 'all-configured')

  const historicalFixedTokens = 7_221
  const output = {
    estimator: 'ceil(characters / 4)',
    projectRoot,
    historicalReference: {
      fixedTokensBeforeHotColdOptimization: historicalFixedTokens,
      currentHotFixedTokens: hot.fixedTokens,
      savedTokens: historicalFixedTokens - hot.fixedTokens,
      reductionPercent: Number((((historicalFixedTokens - hot.fixedTokens) / historicalFixedTokens) * 100).toFixed(1)),
    },
    defaultSystemPromptContainsSkill: hot.systemPrompt.includes('prompt-cache-optimizer'),
    hot: publicSnapshot(hot, hot),
    scenarios: scenarios.map((item) => publicSnapshot(item, hot)),
    allConfigured: publicSnapshot(allConfigured, hot),
  }
  process.stdout.write(`${JSON.stringify(output, null, 2)}\n`)
} finally {
  await runtime?.dispose()
  await rm(temporaryDataDirectory, { recursive: true, force: true })
}
