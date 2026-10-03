import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { WorkflowService } from '../services/workflow-service.mjs'
import {
  validateWorkflowInputDefinitions,
  validateWorkflowInputs,
  renderWorkflowTemplate,
  parseWorkflowMedia,
} from '../../shared/workflow/workflow-inputs.mjs'

const definition = (name, type = 'text', required = true) => ({
  id: name,
  name,
  label: name,
  type,
  required,
  defaultValue: '',
  description: '',
})

test('workflow values validate whitespace, finite numbers, exact booleans, limits and unknown keys', () => {
  const inputs = [
    definition('task'),
    definition('count', 'number'),
    definition('confirmed', 'boolean'),
  ]
  assert.deepEqual(
    validateWorkflowInputs(inputs, { task: 'create', count: '0', confirmed: false }),
    { task: 'create', count: 0, confirmed: false },
  )
  assert.throws(() => validateWorkflowInputs(inputs, { task: '  ', count: 1, confirmed: true }), {
    code: 'workflow_input_required',
    inputName: 'task',
  })
  for (const count of [true, {}, 'Infinity', '0x12', 'bad'])
    assert.throws(() => validateWorkflowInputs([definition('count', 'number')], { count }), {
      code: 'workflow_input_type',
    })
  for (const confirmed of [1, 'yes', {}])
    assert.throws(
      () => validateWorkflowInputs([definition('confirmed', 'boolean')], { confirmed }),
      { code: 'workflow_input_type' },
    )
  assert.throws(() => validateWorkflowInputs([], { task: 'a'.repeat(16001) }), {
    code: 'workflow_input_too_long',
  })
  assert.throws(() => validateWorkflowInputs([], { unknown: 'value' }), {
    code: 'workflow_input_unknown',
  })
  assert.deepEqual(validateWorkflowInputs([], { task: 'one-off task' }), { task: 'one-off task' })
  assert.deepEqual(validateWorkflowInputs([]), {})
})

test('workflow definitions reject ambiguous or unsafe names and validate optional media defaults', () => {
  for (const name of ['__proto__', 'constructor', 'prototype', 'bad.name', 'a'.repeat(81)])
    assert.throws(() => validateWorkflowInputDefinitions([definition(name)]), {
      code: 'workflow_input_unsafe_name',
    })
  assert.throws(() => validateWorkflowInputDefinitions([definition('same'), definition('same')]), {
    code: 'workflow_input_duplicate_name',
  })
  assert.throws(() => validateWorkflowInputDefinitions([{ ...definition('task'), label: '' }]), {
    code: 'workflow_input_invalid_definition',
  })
  const media = { id: 'source', name: 'source.png', mimeType: 'image/png', size: 1024 }
  const inputs = [{ ...definition('image', 'image', false), defaultValue: media }]
  assert.deepEqual(validateWorkflowInputs(inputs), { image: media })
  assert.deepEqual(validateWorkflowInputs([definition('video', 'video', false)]), { video: null })
  assert.throws(() => parseWorkflowMedia({ ...media, path: '/private/path' }), {
    code: 'workflow_input_type',
  })
  assert.throws(() => validateWorkflowInputs([definition('video', 'video')], { video: media }), {
    code: 'workflow_input_type',
  })
})

test('template substitution is single-pass, uses own properties and fails unknown references', () => {
  assert.equal(
    renderWorkflowTemplate('{{inputs.task}} / {{inputs.other}}', {
      inputs: { task: '{{inputs.other}}', other: '$& safe' },
    }),
    '{{inputs.other}} / $& safe',
  )
  assert.equal(
    renderWorkflowTemplate('{{nodes.first.output.count}} {{previous.summary}}', {
      nodes: { first: { output: { count: 0 } } },
      previous: { summary: 'done' },
    }),
    '0 done',
  )
  assert.throws(() => renderWorkflowTemplate('{{inputs.missing}}', { inputs: {} }), {
    code: 'workflow_template_unknown',
  })
  assert.throws(() => renderWorkflowTemplate('{{inputs.constructor}}', { inputs: {} }), {
    code: 'workflow_template_unknown',
  })
})

async function fixture(t, options = {}) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-workflow-inputs-'))
  const complete = Promise.withResolvers()
  const messages = []
  const service = new WorkflowService({
    path: join(dataDir, 'workflows.json'),
    cwd: dataDir,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => {},
      prompt: async (input) => {
        messages.push(input)
        return {
          text: messages.length === 1 ? '{"value":"upstream"}' : 'done',
          sessionId: 'test',
          assets: [],
        }
      },
    },
    notifications: { notify: async () => complete.resolve() },
    ...options,
  })
  await service.init()
  t.after(async () => {
    await service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  return { service, messages, complete }
}

test('each run expands agent inputs, previous and node outputs without modifying workflow defaults', async (t) => {
  const { service, messages, complete } = await fixture(t)
  const workflow = await service.create({
    name: 'Inputs',
    status: 'published',
    notifications: ['browser'],
    inputs: [{ ...definition('task'), defaultValue: 'default' }],
    nodes: [
      { id: 'first', kind: 'prompt', prompt: 'Task: {{inputs.task}}', outputFormat: 'json' },
      {
        id: 'second',
        kind: 'prompt',
        prompt: '{{inputs.task}} / {{previous.output.value}} / {{nodes.first.output.value}}',
      },
    ],
  })
  const supplied = { task: 'specific' }
  const run = await service.runNow(workflow.id, { inputs: supplied })
  supplied.task = 'changed by caller'
  await complete.promise
  assert.equal(service.getRun(run.id).status, 'completed')
  assert.equal(service.getRun(run.id).inputs.task, 'specific')
  assert.match(messages[0].message, /Task: specific/)
  assert.match(messages[1].message, /specific \/ upstream \/ upstream/)
  assert.equal(service.getState().workflows[0].inputs[0].defaultValue, 'default')
  assert.equal(service.getState().workflows[0].nodes[0].prompt, 'Task: {{inputs.task}}')
})

test('unknown template variables and inputs fail before a run or Agent call is created', async (t) => {
  const { service, messages } = await fixture(t)
  const workflow = await service.create({
    name: 'Unknown',
    status: 'published',
    nodes: [{ id: 'first', kind: 'prompt', prompt: '{{inputs.unknown}}' }],
  })
  await assert.rejects(service.runNow(workflow.id), { code: 'workflow_template_unknown' })
  await assert.rejects(service.runNow(workflow.id, { inputs: { unrelated: 'x' } }), {
    code: 'workflow_input_unknown',
  })
  assert.equal(service.getState().runs.length, 0)
  assert.equal(messages.length, 0)
})

test('asynchronous media preflight reserves the run slot and shutdown prevents a late start', async (t) => {
  const gate = Promise.withResolvers()
  const entered = Promise.withResolvers()
  const { service, messages } = await fixture(t, {
    resolveMediaInputs: async () => {
      entered.resolve()
      await gate.promise
      return { attachments: [], context: '' }
    },
  })
  const workflow = await service.create({
    name: 'Reserved',
    status: 'published',
    nodes: [{ id: 'first', kind: 'prompt', prompt: 'task' }],
  })
  const pending = service.runNow(workflow.id)
  await entered.promise
  await assert.rejects(service.runNow(workflow.id), /已经在运行/)
  await assert.rejects(service.update(workflow.id, { name: 'Changed' }), /暂时不能修改/)
  await assert.rejects(service.remove(workflow.id), /暂时不能删除/)
  const closing = service.dispose()
  const rejected = assert.rejects(pending, { code: 'WORKFLOW_CANCELLED' })
  gate.resolve()
  await rejected
  await closing
  assert.equal(messages.length, 0)
  assert.equal(service.getState().runs.length, 0)
})
