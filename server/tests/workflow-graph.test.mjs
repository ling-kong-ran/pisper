import assert from 'node:assert/strict'
import test from 'node:test'
import {
  analyzeWorkflowGraph,
  createLinearWorkflowEdges,
  normalizeWorkflowEdges,
  wouldCreateWorkflowCycle,
} from '../../shared/workflow-graph.mjs'

const nodes = [
  { id: 'trigger', kind: 'trigger', enabled: true },
  { id: 'prepare', kind: 'prompt', enabled: true },
  { id: 'finish', kind: 'notification', enabled: true },
]

test('workflow graph creates stable linear edges and preserves node order', () => {
  const edges = createLinearWorkflowEdges(nodes, (index) => `edge-${index}`)
  assert.deepEqual(edges, [
    {
      id: 'edge-0',
      source: 'trigger',
      sourcePort: 'output',
      target: 'prepare',
      targetPort: 'input',
    },
    {
      id: 'edge-1',
      source: 'prepare',
      sourcePort: 'output',
      target: 'finish',
      targetPort: 'input',
    },
  ])
  assert.deepEqual(
    analyzeWorkflowGraph(nodes, edges).order.map((node) => node.id),
    ['trigger', 'prepare', 'finish'],
  )
})

test('workflow graph normalization removes invalid and duplicate connections', () => {
  const edges = normalizeWorkflowEdges(
    [
      { id: 'valid', source: 'trigger', sourcePort: 'true', target: 'prepare' },
      { id: 'duplicate', source: 'trigger', sourcePort: 'true', target: 'prepare' },
      { id: 'self', source: 'prepare', target: 'prepare' },
      { id: 'missing', source: 'missing', target: 'finish' },
      { id: 'fallback-port', source: 'prepare', sourcePort: 'invalid', target: 'finish' },
    ],
    nodes,
  )
  assert.deepEqual(edges, [
    {
      id: 'valid',
      source: 'trigger',
      sourcePort: 'true',
      target: 'prepare',
      targetPort: 'input',
    },
    {
      id: 'fallback-port',
      source: 'prepare',
      sourcePort: 'output',
      target: 'finish',
      targetPort: 'input',
    },
  ])
})

test('workflow graph cycle checks ignore disabled nodes but reject active back edges', () => {
  const edges = createLinearWorkflowEdges(nodes)
  assert.equal(wouldCreateWorkflowCycle(nodes, edges, 'finish', 'trigger'), true)
  assert.equal(wouldCreateWorkflowCycle(nodes, edges, 'trigger', 'finish'), false)

  const withDisabledCycle = analyzeWorkflowGraph(
    [...nodes, { id: 'disabled', kind: 'prompt', enabled: false }],
    [
      ...edges,
      { id: 'finish-disabled', source: 'finish', target: 'disabled' },
      { id: 'disabled-trigger', source: 'disabled', target: 'trigger' },
    ],
  )
  assert.equal(withDisabledCycle.hasCycle, false)
  assert.deepEqual(
    withDisabledCycle.nodes.map((node) => node.id),
    ['trigger', 'prepare', 'finish'],
  )
})
