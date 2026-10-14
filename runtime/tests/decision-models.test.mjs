import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { DecisionService } from '../services/decision-service.mjs'
import { createDecisionRegistry } from '../services/decision-registry.mjs'
import { jevDecisionAdapter } from '../services/decision-jev-adapter.mjs'
import { defaultDecisionRegistry } from '../services/decision-backends.mjs'
import { DECISION_PROVIDER_CATALOG } from '../../shared/decision-provider-catalog.mjs'

const all = { questionTypes: ['boolean', 'choice', 'score'], booleanProbability: true }
const alternateProvider = {
  protocol: 'example-classifier',
  defaultBaseUrl: 'https://example.invalid',
  path: '/v2/classify',
  defaultModelId: 'model-a',
}

// 第二种协议使用 observation/prompts 和 ratings，不依赖 SDK 或 Jev 的问题/答案名称。
function fixtureRegistry({ onRequest = () => {}, respond, capabilities = all } = {}) {
  const adapter = {
    id: 'example-classifier',
    capabilities,
    async decide(config, input, context) {
      const request = {
        observation: input.state,
        prompts: Object.entries(input.questions).map(([key, q]) => ({
          key,
          kind: q.type,
          label: q.instructions,
          options: q.options,
        })),
      }
      onRequest({ config, request, signal: context.signal })
      if (respond) return respond(config, input, context)
      const response = { ratings: request.prompts.map((q) => ({ key: q.key, yes: 0.97 })) }
      return {
        model: config.modelId,
        usage: { inputTokens: 1, costUsd: null },
        answers: Object.fromEntries(
          response.ratings.map((r) => {
            const q = input.questions[r.key]
            if (q.type === 'choice')
              return [
                r.key,
                { type: 'choice', choice: q.options[0], confidence: null, probabilities: {} },
              ]
            if (q.type === 'score')
              return [
                r.key,
                { type: 'score', score: 0, confidence: null, probabilities: {}, legend: null },
              ]
            return [
              r.key,
              {
                type: 'boolean',
                value: true,
                probabilityTrue: capabilities.booleanProbability ? r.yes : null,
              },
            ]
          }),
        ),
      }
    },
  }
  return createDecisionRegistry({
    adapters: [jevDecisionAdapter, adapter],
    providers: { ...DECISION_PROVIDER_CATALOG, alternate: alternateProvider },
    models: [
      { provider: 'alternate', modelId: 'model-a', approvalPolicyId: 'example-calibration-a' },
      { provider: 'alternate', modelId: 'model-b', approvalPolicyId: 'example-calibration-b' },
      {
        provider: 'alternate',
        modelId: 'choice-only',
        capabilities: { questionTypes: ['choice'], booleanProbability: false },
      },
    ],
  })
}

async function makeService(t, registry = fixtureRegistry(), stored) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-model-contract-'))
  if (stored) {
    await mkdir(join(dataDir, 'decisions'))
    await writeFile(join(dataDir, 'decisions/config.json'), JSON.stringify(stored))
  }
  const service = new DecisionService({ dataDir, registry })
  await service.init()
  t.after(async () => {
    await service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  return service
}
async function configure(service, modelId = 'model-a') {
  await service.updateConfig({
    remote: { provider: 'alternate', apiKey: 'synthetic', modelId },
    delegate: { enabled: true, allowThreshold: 0.9 },
  })
}
const question = { type: 'noul', instructions: 'Is this positive?' }
const call = { toolName: 'read', args: { file: 'example.txt' } }

test('second protocol serves v1 decisions, connection tests, action verification and approval without Jev fields', async (t) => {
  const requests = []
  const service = await makeService(t, fixtureRegistry({ onRequest: (r) => requests.push(r) }))
  await configure(service)
  const result = await service.decide({ state: 'sample', questions: [question] })
  assert.deepEqual(result, {
    backend: 'remote',
    model: 'model-a',
    usage: { inputTokens: 1, costUsd: null },
    answers: { q0: { type: 'noul', noul: 0.97 } },
  })
  assert.equal((await service.testConnection()).ok, true)
  assert.equal(
    (await service.verifyActionOutcome({ expectation: 'ready', outcomeText: 'ready' })).passed,
    true,
  )
  assert.equal((await service.judgeToolCall(call)).verdict, 'approve')
  for (const r of requests) {
    assert.equal(r.config.endpoint, 'https://example.invalid/v2/classify')
    assert.equal(r.request.prompts[0].kind, 'boolean')
    assert.ok(!JSON.stringify(r.request).includes('noul'))
  }
  assert.equal(service.publicConfig().approval.status, 'ready')
})

test('same-protocol unknown model works for ordinary decisions but cannot inherit approval', async (t) => {
  let count = 0
  const service = await makeService(t, fixtureRegistry({ onRequest: () => count++ }))
  await configure(service, 'future-model')
  assert.equal(
    (await service.decide({ state: 'sample', questions: [question] })).answers.q0.noul,
    0.97,
  )
  assert.deepEqual(await service.judgeToolCall(call), { verdict: 'ask' })
  assert.equal(count, 1)
  assert.equal(service.publicConfig().approval.status, 'model_unverified')
})

test('model capabilities reject unsupported requests before sending and connection test uses a supported primitive', async (t) => {
  let count = 0
  const service = await makeService(t, fixtureRegistry({ onRequest: () => count++ }))
  await configure(service, 'choice-only')
  await assert.rejects(service.decide({ state: 'sample', questions: [question] }), {
    code: 'unsupported_capability',
  })
  assert.equal(count, 0)
  assert.equal((await service.testConnection()).ok, true)
  await service.updateConfig({ delegate: { verifyActions: true } })
  assert.equal(service.actionVerificationEnabled(), false)
})

test('boolean labels without probabilities cannot become v1 probabilities or approvals', async (t) => {
  const service = await makeService(
    t,
    fixtureRegistry({ capabilities: { questionTypes: ['boolean'], booleanProbability: false } }),
  )
  await configure(service)
  assert.equal((await service.testConnection()).ok, true)
  await assert.rejects(service.decide({ state: 'sample', questions: [question] }), {
    code: 'unsupported_capability',
  })
  assert.deepEqual(await service.judgeToolCall(call), { verdict: 'ask' })
})

test('all adapters must pass common answer, probability and usage validation', async (t) => {
  const valid = {
    model: 'model-a',
    answers: { q0: { type: 'boolean', value: true, probabilityTrue: 0.97 } },
    usage: { inputTokens: 1, costUsd: null },
  }
  for (const payload of [
    { ...valid, answers: { other: valid.answers.q0 } },
    { ...valid, answers: { q0: { ...valid.answers.q0, probabilityTrue: 1.1 } } },
    { ...valid, answers: { q0: { ...valid.answers.q0, value: false } } },
    { ...valid, usage: { inputTokens: NaN, costUsd: null } },
  ]) {
    const service = await makeService(t, fixtureRegistry({ respond: async () => payload }))
    await configure(service)
    await assert.rejects(service.decide({ state: 'sample', questions: [question] }), {
      code: 'bad_response',
    })
  }
})

test('new SDK exceptions are redacted at the registry boundary', async (t) => {
  const service = await makeService(
    t,
    fixtureRegistry({
      respond: async () => {
        throw new Error('synthetic-api-secret')
      },
    }),
  )
  await configure(service)
  await assert.rejects(
    service.decide({ state: 'sample', questions: [question] }),
    (error) =>
      error.code === 'bad_response' &&
      !String(error).includes('synthetic-api-secret') &&
      !error.cause,
  )
})

test('switching model or endpoint requires an explicit threshold binding and survives restart', async (t) => {
  const registry = fixtureRegistry()
  const service = await makeService(t, registry)
  await configure(service)
  await service.updateConfig({ remote: { modelId: 'model-b' } })
  assert.equal(service.publicConfig().approval.status, 'threshold_required')
  assert.deepEqual(await service.judgeToolCall(call), { verdict: 'ask' })
  const stored = JSON.parse(await readFile(service.configPath, 'utf8'))
  assert.equal(stored.approvalBinding.modelId, 'model-a')
  const reloaded = await makeService(t, registry, stored)
  assert.equal(reloaded.publicConfig().approval.status, 'threshold_required')
  await reloaded.updateConfig({ delegate: { allowThreshold: 0.9 } })
  assert.equal((await reloaded.judgeToolCall(call)).verdict, 'approve')
  await reloaded.updateConfig({ remote: { baseUrl: 'https://relay.invalid' } })
  assert.equal(reloaded.publicConfig().approval.status, 'threshold_required')
  assert.deepEqual(await reloaded.judgeToolCall(call), { verdict: 'ask' })
})

test('legacy v1 config binds its original model without exposing credentials or rewriting on read', async (t) => {
  const stored = {
    version: 1,
    remote: {
      provider: 'typesafe',
      baseUrl: '',
      modelId: 'jev-1.13.0',
      apiKey: 'synthetic-private',
    },
    delegate: { enabled: true, allowThreshold: 0.95, verifyActions: false },
  }
  const service = await makeService(t, defaultDecisionRegistry, stored)
  assert.equal(service.publicConfig().approval.status, 'ready')
  assert.equal(JSON.stringify(service.publicConfig()).includes('synthetic-private'), false)
  assert.equal(JSON.parse(await readFile(service.configPath, 'utf8')).approvalBinding, undefined)
  await service.updateConfig({ remote: { modelId: 'future-jev' } })
  const saved = JSON.parse(await readFile(service.configPath, 'utf8'))
  assert.equal(saved.approvalBinding.modelId, 'jev-1.13.0')
  assert.equal(service.publicConfig().approval.status, 'model_unverified')
})

test('an approval arriving after a model or threshold change is discarded', async (t) => {
  for (const patch of [
    { remote: { modelId: 'model-b' }, delegate: { allowThreshold: 0.9 } },
    { delegate: { allowThreshold: 0.99 } },
  ]) {
    const started = Promise.withResolvers()
    const finish = Promise.withResolvers()
    const service = await makeService(
      t,
      fixtureRegistry({
        respond: async (config) => {
          started.resolve()
          await finish.promise
          return {
            model: config.modelId,
            answers: { approve: { type: 'boolean', value: true, probabilityTrue: 0.97 } },
            usage: { inputTokens: 1, costUsd: null },
          }
        },
      }),
    )
    await configure(service)
    const pending = service.judgeToolCall(call)
    await started.promise
    await service.updateConfig(patch)
    finish.resolve()
    assert.deepEqual(await pending, { verdict: 'ask' })
  }
})

test('alternate adapter observes caller cancellation and service shutdown with no remaining request', async (t) => {
  for (const shutdown of [false, true]) {
    const started = Promise.withResolvers()
    const service = await makeService(
      t,
      fixtureRegistry({
        respond: async (_config, _input, { signal }) =>
          new Promise((resolve, reject) => {
            signal.addEventListener('abort', () => reject(signal.reason), { once: true })
            started.resolve()
          }),
      }),
    )
    await configure(service)
    const controller = new AbortController()
    const pending = assert.rejects(
      service.decide({ state: 'sample', questions: [question] }, { signal: controller.signal }),
      { code: 'aborted' },
    )
    await started.promise
    if (shutdown) await service.dispose()
    else controller.abort()
    await pending
    assert.equal(service.inFlight.size, 0)
  }
})

test('provider lookup rejects unknown names instead of sending credentials to a fallback provider', async (t) => {
  const service = await makeService(t)
  await assert.rejects(service.updateConfig({ remote: { provider: 'constructor' } }), {
    code: 'unsupported_provider',
  })
  assert.equal(service.publicConfig().remote.provider, 'typesafe')
})

test('registry deadline cancels another protocol rather than relying on Jev transport timeout', async () => {
  const registry = fixtureRegistry({
    respond: async (_config, _input, { signal }) =>
      new Promise((_resolve, reject) => {
        signal.addEventListener('abort', () => reject(signal.reason), { once: true })
      }),
  })
  await assert.rejects(
    registry.decide(
      { provider: 'alternate', baseUrl: '', modelId: 'model-a', apiKey: 'synthetic' },
      { state: 'sample', questions: { q: { type: 'boolean', instructions: '?' } } },
      { timeoutMs: 15 },
    ),
    { code: 'timeout' },
  )
})

test('concurrent model and threshold changes serialize and restart with a matching binding', async (t) => {
  const service = await makeService(t)
  await configure(service)
  await Promise.all([
    service.updateConfig({ remote: { modelId: 'model-b' } }),
    service.updateConfig({ delegate: { allowThreshold: 0.95 } }),
  ])
  const stored = JSON.parse(await readFile(service.configPath, 'utf8'))
  assert.equal(stored.remote.modelId, 'model-b')
  assert.equal(stored.approvalBinding.modelId, 'model-b')
  assert.equal(stored.delegate.allowThreshold, 0.95)
  const reloaded = await makeService(t, fixtureRegistry(), stored)
  assert.equal(reloaded.publicConfig().approval.status, 'ready')
  assert.equal((await reloaded.judgeToolCall(call)).verdict, 'approve')
})

test('client accepts legacy status and validates new approval states without copying secrets', async () => {
  const { parseDecisionsStatus, REMOTE_PROVIDER_PRESETS } =
    await import('../../src/features/decisions/decisions-api.ts')
  const config = {
    remote: {
      provider: 'typesafe',
      baseUrl: 'https://api.typesafe.ai',
      modelId: 'jev-1.13.0',
      hasKey: true,
      apiKey: 'synthetic-private',
    },
    delegate: { enabled: true, allowThreshold: 0.9, verifyActions: false },
  }
  assert.equal(parseDecisionsStatus({ config }).config.approval, undefined)
  assert.equal(
    JSON.stringify(parseDecisionsStatus({ config })).includes('synthetic-private'),
    false,
  )
  for (const status of ['ready', 'model_unverified', 'threshold_required'])
    assert.equal(
      parseDecisionsStatus({ config: { ...config, approval: { status } } }).config.approval.status,
      status,
    )
  assert.throws(
    () => parseDecisionsStatus({ config: { ...config, approval: { status: 'safe' } } }),
    { kind: 'protocol' },
  )
  for (const [provider, preset] of Object.entries(DECISION_PROVIDER_CATALOG)) {
    assert.deepEqual(REMOTE_PROVIDER_PRESETS[provider], {
      baseUrl: preset.defaultBaseUrl,
      modelId: preset.defaultModelId,
    })
  }
})

test('built-in Jev approval policies match exact model IDs, never future versions by prefix', () => {
  for (const [provider, preset] of Object.entries(DECISION_PROVIDER_CATALOG)) {
    const config = {
      provider,
      baseUrl: preset.defaultBaseUrl || 'https://relay.invalid',
      apiKey: 'synthetic',
      modelId: preset.defaultModelId,
    }
    assert.equal(defaultDecisionRegistry.resolve(config).approvalPolicy.policyId, 'legacy-jev-v1')
    assert.equal(
      defaultDecisionRegistry.resolve({ ...config, modelId: `${preset.defaultModelId}-next` })
        .approvalPolicy,
      null,
    )
  }
})
