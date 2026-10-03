import assert from 'node:assert/strict'
import test from 'node:test'
import {
  normalizeWorkflowImageSettings,
  WORKFLOW_IMAGE_NODE_KINDS,
} from '../../shared/workflow/workflow-image-nodes.mjs'
import {
  createWorkflowNode,
  templateWorkflow,
  WORKFLOW_TEMPLATES,
  WORKFLOW_PALETTE,
  workflowImageRequestCount,
} from '../../src/features/workflows/model/workflow-templates.ts'

test('sprite template is an editable workflow graph with four action branches and one export', () => {
  const template = WORKFLOW_TEMPLATES.find((item) => item.id === 'sprite')
  assert.ok(template)
  const workflow = templateWorkflow(template)
  const second = templateWorkflow(template)
  const ids = new Set(workflow.nodes.map((node) => node.id))
  assert.equal(ids.size, workflow.nodes.length)
  assert.ok(second.nodes.every((node) => !ids.has(node.id)))
  assert.ok(workflow.edges.every((edge) => ids.has(edge.source) && ids.has(edge.target)))
  assert.equal(workflow.inputs.find((input) => input.name === 'reference')?.type, 'image')
  assert.equal(workflow.inputs.find((input) => input.name === 'reference')?.required, true)
  const source = workflow.nodes.find((node) => node.kind === 'media-input')
  const preview = workflow.nodes.find((node) => node.kind === 'media-preview')
  const generators = workflow.nodes.filter((node) => node.kind === 'media-generate')
  assert.equal(generators.length, 4)
  assert.equal(workflow.edges.filter((edge) => edge.source === source.id).length, 4)
  assert.equal(workflow.edges.filter((edge) => edge.target === preview.id).length, 4)
  for (const generator of generators) {
    assert.equal(generator.image.frameCount, 4)
    assert.equal(generator.image.directions.length, 8)
    assert.ok(generator.prompt.includes('{{inputs.task}}'))
    let current = generator.id
    for (const kind of [
      'media-background',
      'media-frames',
      'media-transform',
      'media-preview',
      'media-export',
    ]) {
      const edge = workflow.edges.find((edge) => edge.source === current)
      const next = workflow.nodes.find((node) => node.id === edge?.target)
      assert.equal(next?.kind, kind)
      current = next.id
    }
  }
  workflow.nodes.find((node) => node.kind === 'media-generate').image.directions.pop()
  assert.equal(
    template.nodes.find((node) => node.kind === 'media-generate').image.directions.length,
    8,
  )
})

test('image request estimate follows editable directions and enabled action nodes', () => {
  const template = WORKFLOW_TEMPLATES.find((item) => item.id === 'sprite')
  const workflow = templateWorkflow(template)
  assert.equal(workflowImageRequestCount(workflow), 32)
  const generators = workflow.nodes.filter((node) => node.kind === 'media-generate')
  generators[0].image.directions = ['S']
  assert.equal(workflowImageRequestCount(workflow), 25)
  generators[1].enabled = false
  assert.equal(workflowImageRequestCount(workflow), 17)
  const custom = createWorkflowNode(
    'custom-action',
    'media-generate',
    'Jump',
    'Jump over a gap',
    0,
    0,
  )
  workflow.nodes.push(custom)
  assert.equal(workflowImageRequestCount(workflow), 18)
})

test('all image operations can be added independently with valid shared settings', () => {
  for (const kind of WORKFLOW_IMAGE_NODE_KINDS) {
    assert.ok(WORKFLOW_PALETTE.some((item) => item.kind === kind))
    const node = createWorkflowNode(kind, kind, kind, '', 0, 0)
    assert.deepEqual(node.image, normalizeWorkflowImageSettings())
  }
})
