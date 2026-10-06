import assert from 'node:assert/strict'
import { readFile, readdir } from 'node:fs/promises'
import { join, resolve } from 'node:path'

const terminal = new Set(['completed', 'failed', 'cancelled', 'interrupted'])
const contentText = (content) =>
  typeof content === 'string'
    ? content
    : (content || [])
        .filter((part) => part.type === 'text')
        .map((part) => part.text)
        .join('')

// Root owns the loopback model fixture and authenticated backend. This module
// creates only its own records; it does not boot processes or edit configuration.
export async function checkWorkflowParity({
  check,
  json,
  request,
  workspace,
  agent,
  providerId,
  modelId,
  heldModelRequests,
  waitForHeldModel,
  delay,
}) {
  assert.equal(typeof agent, 'string', 'Pass the isolated agent directory for JSONL proof')
  const model = { provider: providerId, model: modelId }
  const cwd = resolve(workspace)

  function cleanupError(original, cleanup) {
    return original
      ? new AggregateError(
          [original, cleanup],
          `Original case failed: ${original.stack || original}\nOwned cleanup also failed: ${cleanup.stack || cleanup}`,
        )
      : cleanup
  }

  async function poll(read, predicate, label, timeout = 20000) {
    const deadline = Date.now() + timeout
    let latest
    while (Date.now() < deadline) {
      latest = await read(Math.max(1, Math.min(5000, deadline - Date.now())))
      if (predicate(latest)) return latest
      await delay(50)
    }
    assert.fail(`${label} did not settle within ${timeout}ms: ${JSON.stringify(latest)}`)
  }

  function response(path, method, body) {
    return request(path, {
      method,
      ...(body === undefined
        ? {}
        : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(5000),
    })
  }

  async function expectError(path, method, body, status, code) {
    const result = await response(path, method, body)
    const data = await result.json()
    assert.equal(result.status, status, `${method} ${path}: ${JSON.stringify(data)}`)
    assert.equal(data.code, code)
    assert.equal(typeof data.error, 'string')
  }

  async function removeRecord(path) {
    await poll(
      async () => {
        const result = await response(path, 'DELETE')
        if (result.status === 404) return true
        const data = await result.json()
        if (result.status === 409) return false
        assert.equal(result.status, 200, `Owned record cleanup failed: ${JSON.stringify(data)}`)
        assert.equal(data.deleted, true)
        return true
      },
      Boolean,
      `Delete owned ${path}`,
      5000,
    )
  }

  async function journal(sessionId) {
    const root = join(resolve(agent), 'sessions')
    const entries = await readdir(root, { withFileTypes: true })
    const files = entries.filter((entry) => entry.isFile()).map((entry) => join(root, entry.name))
    for (const entry of entries.filter((entry) => entry.isDirectory())) {
      const nested = await readdir(join(root, entry.name), { withFileTypes: true })
      files.push(
        ...nested.filter((item) => item.isFile()).map((item) => join(root, entry.name, item.name)),
      )
    }
    for (const file of files.filter((file) => file.endsWith('.jsonl'))) {
      const lines = (await readFile(file, 'utf8')).trim().split('\n')
      const header = JSON.parse(lines[0])
      if (header.type !== 'session' || header.id !== sessionId) continue
      const parsed = lines.map((line) => JSON.parse(line))
      return parsed.filter((entry) => entry.type === 'message').map((entry) => entry.message)
    }
    assert.fail(`Workflow public session ${sessionId} has no persisted JSONL in the isolated agent`)
  }

  async function sessionProof(sessionId, prompts, outputs) {
    assert.ok(sessionId, 'A real Pi session ID must be published')
    const sessions = await json('/api/sessions')
    const summary = sessions.sessions.find((session) => session.id === sessionId)
    assert.ok(summary, 'Workflow session must be visible in the public session catalog')
    assert.equal(summary.executionMode, 'full-access')
    const selected = await json(`/api/sessions/${sessionId}/model`)
    assert.equal(selected.provider, providerId)
    assert.equal(selected.id, modelId)
    const history = await json(`/api/sessions/${sessionId}/messages?limit=200`)
    assert.ok(Array.isArray(history.messages))
    for (const prompt of prompts) {
      assert.ok(
        history.messages.some((entry) => entry.role === 'user' && entry.text.includes(prompt)),
      )
    }
    const assistant = history.messages.filter((entry) => entry.role === 'agent')
    assert.ok(
      assistant.length >= outputs.length,
      'Every completed prompt must persist its assistant turn',
    )
    for (const output of outputs) {
      assert.ok(
        typeof output === 'string' && output.trim(),
        'Completed nodes need actual model output',
      )
      assert.ok(assistant.some((entry) => entry.text.trim() === output.trim()))
    }
    const persisted = await journal(sessionId)
    const persistedAssistant = persisted.filter((entry) => entry.role === 'assistant')
    assert.ok(persistedAssistant.length >= outputs.length)
    for (const output of outputs) {
      assert.ok(
        persistedAssistant.some((entry) => contentText(entry.content).trim() === output.trim()),
      )
    }
    for (const prompt of prompts) {
      assert.ok(
        persisted.some(
          (entry) => entry.role === 'user' && contentText(entry.content).includes(prompt),
        ),
      )
    }
    return { sessionId, historyMessages: history.messages.length, jsonlMessages: persisted.length }
  }

  async function withWorkflow(name, nodes, edges, runTest, options = {}) {
    let workflow
    let failure
    const runs = new Set()
    const sessions = new Set()
    const remember = (run) => {
      if (run.sessionId) sessions.add(run.sessionId)
      for (const node of run.nodes || []) if (node.sessionId) sessions.add(node.sessionId)
      return run
    }
    try {
      const created = await json('/api/workflows', 'POST', {
        name: `native-workflow-smoke-${name}`,
        status: 'published',
        cwd,
        model,
        notifications: [],
        nodes,
        edges,
        ...options,
      })
      workflow = created.workflow
      assert.ok(workflow.id)
      assert.equal(workflow.status, 'published')
      const run = async (body = {}) => {
        const started = await json(`/api/workflows/${workflow.id}/run`, 'POST', body)
        assert.equal(started.started, true)
        assert.ok(started.run.id)
        runs.add(started.run.id)
        return remember(started.run)
      }
      const wait = (id, status) =>
        poll(
          async (timeout) =>
            remember(await json(`/api/workflow-runs/${id}`, 'GET', undefined, timeout)),
          (current) => {
            if (terminal.has(current.status) && current.status !== status) {
              assert.fail(
                `Expected ${status}, got actual workflow outcome: ${JSON.stringify(current)}`,
              )
            }
            return current.status === status
          },
          `Workflow ${id} ${status}`,
        )
      return await runTest({ workflow, run, wait })
    } catch (error) {
      failure = error
      throw error
    } finally {
      try {
        if (workflow) {
          for (const id of runs) {
            const current = remember(await json(`/api/workflow-runs/${id}`, 'GET', undefined, 5000))
            if (!terminal.has(current.status)) {
              const stopped = await response(`/api/workflow-runs/${id}/stop`, 'POST', {})
              assert.ok(
                [202, 404].includes(stopped.status),
                `Owned run ${id} must stop during cleanup`,
              )
              await poll(
                async (timeout) =>
                  remember(await json(`/api/workflow-runs/${id}`, 'GET', undefined, timeout)),
                (value) => terminal.has(value.status),
                `Cleanup workflow ${id}`,
                5000,
              )
            }
          }
          await removeRecord(`/api/workflows/${workflow.id}`)
          for (const id of sessions) await removeRecord(`/api/sessions/${id}`)
          const dashboard = await json('/api/workflows')
          assert.ok(!dashboard.workflows.some((entry) => entry.id === workflow.id))
          assert.ok(!dashboard.runs.some((entry) => entry.workflowId === workflow.id))
        }
      } catch (error) {
        throw cleanupError(failure, error)
      }
    }
  }

  await check(
    'Workflow HTTP publishes a DAG and persists two real Pi turns in one public session',
    () =>
      withWorkflow(
        'dag',
        [
          { id: 'first', kind: 'prompt', label: 'first', prompt: 'native-workflow-first' },
          {
            id: 'second',
            kind: 'prompt',
            label: 'second',
            prompt: 'native-workflow-second {{nodes.first.output}}',
          },
        ],
        [{ source: 'first', target: 'second' }],
        async ({ workflow, run, wait }) => {
          const dashboard = await json('/api/workflows')
          assert.ok(Array.isArray(dashboard.models) && Array.isArray(dashboard.skills))
          assert.ok(
            dashboard.models.some(
              (entry) => entry.provider === providerId && entry.model === modelId,
            ),
          )
          assert.ok(dashboard.workflows.some((entry) => entry.id === workflow.id))
          const sourceSessionId = 'native-workflow-smoke-source-dag'
          const started = await run({ sourceSessionId })
          const completed = await wait(started.id, 'completed')
          const scoped = await json(`/api/sessions/${sourceSessionId}/workflow-runs`)
          assert.deepEqual(
            scoped.runs.map((entry) => entry.id),
            [completed.id],
          )
          const unrelated = await json(
            '/api/sessions/native-workflow-smoke-unrelated-source/workflow-runs',
          )
          assert.deepEqual(unrelated.runs, [])
          assert.equal(completed.completedNodes, 2)
          assert.equal(completed.totalNodes, 2)
          assert.equal(completed.workflowRevision, workflow.revision)
          const first = completed.nodes.find((entry) => entry.id === 'first')
          const second = completed.nodes.find((entry) => entry.id === 'second')
          assert.equal(first.status, 'completed')
          assert.equal(second.status, 'completed')
          assert.equal(first.attempts, 1)
          assert.equal(second.attempts, 1)
          assert.equal(
            first.sessionId,
            second.sessionId,
            'A single predecessor must inherit its public session',
          )
          const proof = await sessionProof(
            second.sessionId,
            ['native-workflow-first', 'native-workflow-second'],
            [first.output, second.output],
          )
          return {
            workflowId: workflow.id,
            runId: completed.id,
            sourceSessionId,
            scopedRunIsolation: true,
            ...proof,
          }
        },
      ),
  )

  await check(
    'Workflow HTTP approval resolves once and cancellation blocks the next Pi prompt',
    () =>
      withWorkflow(
        'approval',
        [
          {
            id: 'review',
            kind: 'approval',
            approval: { message: 'Synthetic approval', timeoutMinutes: 1 },
          },
          { id: 'after', kind: 'prompt', prompt: 'native-workflow-after-approval' },
        ],
        [{ source: 'review', target: 'after' }],
        async ({ run, wait }) => {
          const started = await run()
          const pending = await wait(started.id, 'waiting_approval')
          assert.equal(pending.nodes.find((entry) => entry.id === 'after').attempts, 0)
          assert.equal(
            pending.nodes.find((entry) => entry.id === 'review').approval.message,
            'Synthetic approval',
          )
          const decision = await json(`/api/workflow-runs/${started.id}/approvals/review`, 'POST', {
            approved: true,
            comment: 'fixture approved',
          })
          assert.equal(decision.resolved, true)
          await expectError(
            `/api/workflow-runs/${started.id}/approvals/review`,
            'POST',
            { approved: true },
            404,
            'workflow_not_found',
          )
          const completed = await wait(started.id, 'completed')
          assert.deepEqual(completed.nodes.find((entry) => entry.id === 'review').output, {
            approved: true,
            comment: 'fixture approved',
          })
          const after = completed.nodes.find((entry) => entry.id === 'after')
          const proof = await sessionProof(
            after.sessionId,
            ['native-workflow-after-approval'],
            [after.output],
          )
          const cancelledStart = await run()
          await wait(cancelledStart.id, 'waiting_approval')
          await json(`/api/workflow-runs/${cancelledStart.id}/stop`, 'POST', {})
          const cancelled = await wait(cancelledStart.id, 'cancelled')
          assert.equal(cancelled.nodes.find((entry) => entry.id === 'after').attempts, 0)
          assert.equal(cancelled.nodes.find((entry) => entry.id === 'after').sessionId, '')
          await expectError(
            `/api/workflow-runs/${cancelledStart.id}/approvals/review`,
            'POST',
            { approved: true },
            404,
            'workflow_not_found',
          )
          return { approvedRun: completed.id, cancelledRun: cancelled.id, ...proof }
        },
      ),
  )

  await check(
    'Stopping an actual held Workflow model stream releases its owner and blocks overlapping mutation',
    async () => {
      const label = 'workflow-stop'
      try {
        return await withWorkflow(
          'stream-stop',
          [{ id: 'held', kind: 'prompt', prompt: `native-parity-hold:${label}` }],
          [],
          async ({ workflow, run, wait }) => {
            assert.equal(workflow.nodes[0].prompt, `native-parity-hold:${label}`)
            const started = await run()
            await poll(
              async (timeout) => {
                const actual = await json(
                  `/api/workflow-runs/${started.id}`,
                  'GET',
                  undefined,
                  timeout,
                )
                if (terminal.has(actual.status)) {
                  const history = actual.sessionId
                    ? await json(
                        `/api/sessions/${actual.sessionId}/messages`,
                        'GET',
                        undefined,
                        timeout,
                      )
                    : { messages: [] }
                  assert.fail(
                    `Workflow reached ${actual.status} before its actual held model connection: ${JSON.stringify(
                      {
                        runId: actual.id,
                        error: actual.error,
                        nodes: actual.nodes,
                        history: history.messages.map(({ role, text }) => ({ role, text })),
                      },
                    )}`,
                  )
                }
                return { held: heldModelRequests.has(label), run: actual }
              },
              ({ held }) => held,
              `Actual workflow model connection ${label}`,
              10000,
            )
            const current = await json(`/api/workflow-runs/${started.id}`)
            assert.ok(
              current.sessionId,
              'The real held model must have a published public session owner',
            )
            await expectError(`/api/workflows/${workflow.id}/run`, 'POST', {}, 409, 'workflow_busy')
            await expectError(
              `/api/workflows/${workflow.id}`,
              'PATCH',
              { description: 'mutation while active' },
              409,
              'workflow_busy',
            )
            await json(`/api/workflow-runs/${started.id}/stop`, 'POST', {}, 5000)
            await waitForHeldModel(label, false, 5000)
            const stopped = await wait(started.id, 'cancelled')
            assert.notEqual(stopped.nodes[0].status, 'completed')
            const live = await json(`/api/sessions/${current.sessionId}/live`)
            assert.equal(live.streaming, false)
            await json(`/api/sessions/${current.sessionId}/model`, 'PUT', model, 5000)
            return { runId: stopped.id, sessionId: current.sessionId, modelConnectionClosed: true }
          },
        )
      } finally {
        heldModelRequests.get(label)?.()
      }
    },
  )

  await check('Workflow unavailable selected model persists failure without a completed node', () =>
    withWorkflow(
      'unavailable-model',
      [{ id: 'fail', kind: 'prompt', prompt: 'native-workflow-unavailable-model' }],
      [],
      async ({ run, wait }) => {
        const failed = await wait((await run()).id, 'failed')
        // Release increments this progress count in executeNode's finally,
        // including a failed node. Actual success is the separate node status.
        assert.equal(failed.completedNodes, 1)
        assert.equal(failed.nodes[0].status, 'failed')
        assert.equal(failed.nodes[0].attempts, 1)
        assert.ok(failed.error.trim())
        assert.equal(failed.nodes[0].output, '')
        const history = await json(`/api/sessions/${failed.sessionId}/messages`)
        assert.ok(!history.messages.some((entry) => entry.role === 'agent' && entry.text.trim()))
        return { runId: failed.id, status: failed.status, modelRejected: true }
      },
      { model: { provider: providerId, model: `${modelId}-unavailable-workflow-fixture` } },
    ),
  )

  await check(
    'Disabled Schedule manual run persists actual Pi output and keeps nextRunAt unchanged',
    async () => {
      let task
      let failure
      const sessions = new Set()
      try {
        const created = await json('/api/schedules', 'POST', {
          name: 'native-schedule-smoke-manual',
          targetType: 'prompt',
          prompt: 'native-schedule-manual-prompt',
          enabled: false,
          frequency: 'interval',
          intervalValue: 1,
          intervalUnit: 'hours',
          timezone: 'Asia/Seoul',
          cwd,
          model,
          notifications: [],
        })
        task = created.task
        assert.equal(task.enabled, false)
        assert.equal(task.nextRunAt, null)
        const started = await json(`/api/schedules/${task.id}/run`, 'POST', {})
        assert.equal(started.started, true)
        const dashboard = await poll(
          (timeout) => json('/api/schedules', 'GET', undefined, timeout),
          (state) => {
            const current = state.runs.find((entry) => entry.taskId === task.id)
            if (current?.sessionId) sessions.add(current.sessionId)
            if (current && terminal.has(current.status) && current.status !== 'completed') {
              assert.fail(`Actual scheduled prompt failed: ${JSON.stringify(current)}`)
            }
            return current?.status === 'completed'
          },
          'Scheduled manual prompt',
        )
        const completed = dashboard.runs.find((entry) => entry.taskId === task.id)
        const current = dashboard.tasks.find((entry) => entry.id === task.id)
        assert.equal(completed.trigger, 'manual')
        assert.equal(current.lastStatus, 'completed')
        assert.equal(current.nextRunAt, null)
        assert.equal(current.lastSummary, completed.summary)
        assert.equal(current.lastError, '')
        assert.ok(completed.finishedAt)
        const proof = await sessionProof(
          completed.sessionId,
          ['native-schedule-manual-prompt'],
          [completed.summary],
        )
        return { taskId: task.id, runId: completed.id, ...proof }
      } catch (error) {
        failure = error
        throw error
      } finally {
        try {
          if (task) {
            // A schedule has no public stop endpoint. Abort only a session whose
            // synthetic title belongs to this task if an earlier assertion failed.
            const catalog = await json('/api/sessions')
            for (const entry of catalog.sessions.filter(
              (entry) => entry.name === `定时任务 · ${task.name}`,
            )) {
              sessions.add(entry.id)
              if (entry.streaming) await json(`/api/sessions/${entry.id}/abort`, 'POST', {}, 5000)
            }
            await removeRecord(`/api/schedules/${task.id}`)
            for (const id of sessions) await removeRecord(`/api/sessions/${id}`)
            const dashboard = await json('/api/schedules')
            assert.ok(!dashboard.tasks.some((entry) => entry.id === task.id))
            assert.ok(!dashboard.runs.some((entry) => entry.taskId === task.id))
          }
        } catch (error) {
          throw cleanupError(failure, error)
        }
      }
    },
  )

  await check(
    'Workflow media binary roundtrip and fixed native image-engine HTTP catalog match their contracts',
    async () => {
      const png = Buffer.from(
        'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aF9kAAAAASUVORK5CYII=',
        'base64',
      )
      const uploaded = await request('/api/workflow-media?name=native-workflow-smoke.png', {
        method: 'POST',
        headers: { 'Content-Type': 'image/png' },
        body: png,
        signal: AbortSignal.timeout(10000),
      })
      assert.equal(uploaded.status, 201)
      const media = await uploaded.json()
      assert.ok(media.id)
      assert.equal(media.name, 'native-workflow-smoke.png')
      assert.equal(media.mimeType, 'image/png')
      assert.equal(media.size, png.length)
      assert.equal(media.data, undefined)
      assert.equal(media.path, undefined)
      const downloaded = await request(`/api/workflow-media/${media.id}/content`, {
        signal: AbortSignal.timeout(10000),
      })
      assert.equal(downloaded.status, 200)
      assert.equal(downloaded.headers.get('content-type'), 'image/png')
      assert.equal(downloaded.headers.get('x-content-type-options'), 'nosniff')
      assert.equal(downloaded.headers.get('content-length'), String(png.length))
      assert.deepEqual(Buffer.from(await downloaded.arrayBuffer()), png)
      const engines = await json('/api/sprite-engines')
      assert.ok(Array.isArray(engines.engines))
      assert.deepEqual(engines.engines.map((entry) => entry.id).sort(), ['background', 'inpaint'])
      for (const [id, version, bytes] of [
        ['background', 'ort-1.20.1_u2netp-v0.0.0', 15906967],
        ['inpaint', '5.0.0-release.1', 13321584],
      ]) {
        const engine = engines.engines.find((entry) => entry.id === id)
        assert.equal(engine.version, version)
        assert.equal(engine.bytes, bytes)
        assert.equal(engine.total, bytes)
        assert.ok(['missing', 'downloading', 'ready', 'failed'].includes(engine.status))
        assert.ok(
          Number.isInteger(engine.received) && engine.received >= 0 && engine.received <= bytes,
        )
        assert.equal(typeof engine.error, 'string')
        assert.equal(typeof engine.file, 'string')
      }
      await expectError(
        '/api/sprite-engines/unknown-fixture/cancel',
        'POST',
        {},
        404,
        'sprite_engine_not_found',
      )
      const imageModels = await json('/api/workflow-image-models')
      assert.ok(Array.isArray(imageModels.models))
      return {
        mediaId: media.id,
        bytes: png.length,
        engines: engines.engines.map(({ id, version, status }) => ({ id, version, status })),
      }
    },
  )
}
