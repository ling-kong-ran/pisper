import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

// All files and HTTP requests belong to the caller's isolated fixture.
export async function checkNotificationParity({
  check,
  json,
  request,
  agent,
  workspace,
  providerId,
  modelId,
  delay,
}) {
  const path = '/api/settings/notifications'
  const empty = '__pisper_browser_events_empty__'
  const content = 'fixture {{chat.title}} / {{chat.summary}} / {{missing.value}}'
  let eventIds

  async function expectError(route, method, body) {
    const response = await request(route, {
      method,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body || {}),
    })
    assert.equal(response.status, 400)
    assert.equal(typeof (await response.json()).error, 'string')
  }

  await check(
    'Notifications use the release config and preserve other provider files',
    async () => {
      const before = await json(path)
      assert.equal(before.templates.length, 6)
      assert.deepEqual(
        before.templates.map((template) => template.id).sort(),
        [
          'chat.completed',
          'chat.waiting',
          'schedule.completed',
          'schedule.failed',
          'workflow.completed',
          'workflow.failed',
        ].sort(),
      )
      for (const template of before.templates) {
        assert.equal(typeof template.name, 'string')
        assert.ok(Array.isArray(template.variables))
        assert.equal(typeof template.channels.browser.content, 'string')
      }
      const others = await Promise.all(
        ['models.json', 'auth.json', 'settings.json'].map((name) => readFile(join(agent, name))),
      )
      const file = join(agent, 'pisper.json')
      const app = JSON.parse(await readFile(file, 'utf8'))
      app.notificationFixture = { keep: true }
      app.notifications = {
        ...app.notifications,
        unknownFixture: { keep: 'nested' },
        browser: { ...app.notifications?.browser, fixture: 'keep' },
      }
      await writeFile(file, JSON.stringify(app))
      assert.equal((await json(`${path}/browser`, 'PATCH', { enabled: {} })).browser.enabled, true)
      assert.equal((await json(`${path}/browser`, 'PATCH', {})).browser.enabled, false)
      assert.equal(
        (await json(`${path}/browser`, 'PATCH', { enabled: true })).browser.enabled,
        true,
      )
      const saved = JSON.parse(await readFile(file, 'utf8'))
      assert.deepEqual(saved.notificationFixture, { keep: true })
      assert.deepEqual(saved.notifications.unknownFixture, { keep: 'nested' })
      assert.equal(saved.notifications.browser.fixture, 'keep')
      assert.equal(saved.notifications.browser.enabled, true)
      const after = await Promise.all(
        ['models.json', 'auth.json', 'settings.json'].map((name) => readFile(join(agent, name))),
      )
      assert.deepEqual(after, others)
      return {
        canonicalConfig: 'pisper.json',
        templates: before.templates.length,
        otherFilesUnchanged: 3,
      }
    },
  )

  await check(
    'Notification template editing and test previews do not enqueue browser events',
    async () => {
      const route = `${path}/templates/chat.completed/browser`
      const before = await json(`${path}/browser/events?after=${empty}`)
      await expectError(route, 'PUT', { content: '   ' })
      await expectError(`${path}/templates/unknown/browser`, 'PUT', { content: 'x' })
      const saved = await json(route, 'PUT', { enabled: true, content })
      assert.equal(
        saved.templates.find((template) => template.id === 'chat.completed').channels.browser
          .content,
        content,
      )
      const preview = await json(`${route}/test`, 'POST', {})
      assert.equal(preview.sent, 1)
      assert.equal(preview.preview, preview.body)
      assert.ok(preview.body.includes('{{missing.value}}'))
      assert.ok(!preview.body.includes('{{chat.title}}'))
      await json(`${path}/browser`, 'PATCH', { enabled: false })
      await expectError(`${route}/test`, 'POST', {})
      await json(`${path}/browser`, 'PATCH', { enabled: true })
      assert.deepEqual(await json(`${path}/browser/events?after=${empty}`), before)
      return { previewOnly: true, unknownVariablesPreserved: true }
    },
  )

  await check('TUI chat reports return 202 and avoid duplicate browser notifications', async () => {
    const before = await json(`${path}/browser/events?after=${empty}`)
    for (const event of ['chat-completed', 'chat-waiting']) {
      const response = await request(`${path}/${event}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ title: 'Fixture', summary: 'complete', reason: 'confirm' }),
      })
      assert.equal(response.status, 202)
      assert.deepEqual(await response.json(), {
        accepted: true,
        systemNotificationEnabled: true,
        channelError: '',
      })
    }
    assert.deepEqual(await json(`${path}/browser/events?after=${empty}`), before)
  })

  await check(
    'Workflow browser notifications are durable and use release UUID cursor semantics',
    async () => {
      let workflow
      let runId
      let evidence
      const failures = []
      const sessions = new Set()
      try {
        workflow = (
          await json('/api/workflows', 'POST', {
            name: 'notification-durable-fixture',
            status: 'published',
            cwd: workspace,
            model: { provider: providerId, model: modelId },
            notifications: [],
            nodes: [
              { id: 'start', kind: 'trigger' },
              { id: 'prompt', kind: 'prompt', prompt: 'notification durable fixture' },
              {
                id: 'first',
                kind: 'notification',
                notification: { title: 'Fixture first', content: 'durable one' },
                notificationTargets: ['browser'],
              },
              {
                id: 'second',
                kind: 'notification',
                notification: { title: 'Fixture second', content: 'durable two' },
                notificationTargets: ['browser'],
              },
            ],
            edges: [
              { source: 'start', target: 'prompt' },
              { source: 'prompt', target: 'first' },
              { source: 'first', target: 'second' },
            ],
          })
        ).workflow
        runId = (await json(`/api/workflows/${workflow.id}/run`, 'POST', {})).run.id
        const deadline = Date.now() + 10000
        let run
        do {
          run = await json(`/api/workflow-runs/${runId}`)
          if (run.sessionId) sessions.add(run.sessionId)
          for (const node of run.nodes || []) if (node.sessionId) sessions.add(node.sessionId)
          if (['completed', 'failed', 'cancelled', 'interrupted'].includes(run.status)) break
          await delay(30)
        } while (Date.now() < deadline)
        assert.equal(run.status, 'completed', JSON.stringify(run))
        const bootstrap = await json(`${path}/browser/events`)
        assert.deepEqual(bootstrap.events, [])
        const fresh = await json(`${path}/browser/events?after=${empty}`)
        assert.equal(fresh.events.length, 2)
        assert.deepEqual(
          fresh.events.map((event) => [event.title, event.body, event.event]),
          [
            ['Fixture first', 'durable one', 'workflow.completed'],
            ['Fixture second', 'durable two', 'workflow.completed'],
          ],
        )
        for (const event of fresh.events) {
          assert.match(event.id, /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i)
          assert.ok(Number.isFinite(Date.parse(event.createdAt)))
        }
        eventIds = fresh.events.map((event) => event.id)
        assert.equal(bootstrap.latestId, eventIds[1])
        assert.deepEqual((await json(`${path}/browser/events?after=${eventIds[0]}`)).events, [
          fresh.events[1],
        ])
        assert.deepEqual((await json(`${path}/browser/events?after=${eventIds[1]}`)).events, [])
        assert.deepEqual(
          (await json(`${path}/browser/events?after=missing-id`)).events,
          fresh.events,
        )
        const stored = JSON.parse(
          await readFile(join(agent, 'pisper-browser-notifications.json'), 'utf8'),
        )
        assert.deepEqual(stored.events, fresh.events)
        evidence = { eventIds, durableFile: 'pisper-browser-notifications.json' }
      } catch (error) {
        failures.push(error)
      } finally {
        try {
          if (runId) {
            let current = await json(`/api/workflow-runs/${runId}`)
            const terminal = (run) =>
              ['completed', 'failed', 'cancelled', 'interrupted'].includes(run.status)
            if (!terminal(current)) {
              const response = await request(`/api/workflow-runs/${runId}/stop`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: '{}',
              })
              assert.ok(
                [200, 202, 404].includes(response.status),
                'Owned notification workflow must stop',
              )
              const deadline = Date.now() + 5000
              while (!terminal(current) && Date.now() < deadline) {
                await delay(30)
                current = await json(`/api/workflow-runs/${runId}`)
              }
              assert.ok(terminal(current), 'Owned notification workflow cancellation must settle')
            }
          }
        } catch (error) {
          failures.push(error)
        }
        try {
          if (workflow) await json(`/api/workflows/${workflow.id}`, 'DELETE')
        } catch (error) {
          failures.push(error)
        }
        for (const id of sessions) {
          try {
            await json(`/api/sessions/${id}`, 'DELETE')
          } catch (error) {
            failures.push(error)
          }
        }
      }
      if (failures.length) throw new AggregateError(failures, failures.map(String).join('\n'))
      return evidence
    },
  )

  return async () => {
    assert.ok(eventIds, 'Durable notification fixture must have completed before restart')
    const state = await json(path)
    assert.equal(state.browser.enabled, true)
    assert.equal(
      state.templates.find((template) => template.id === 'chat.completed').channels.browser.content,
      content,
    )
    const events = await json(`${path}/browser/events?after=${empty}`)
    assert.deepEqual(
      events.events.map((event) => event.id),
      eventIds,
    )
    assert.deepEqual((await json(`${path}/browser/events`)).events, [])
    return { eventIds, templatesPreserved: true }
  }
}
