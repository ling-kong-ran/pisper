const SOURCE_PORTS = new Set(['output', 'true', 'false'])

function nodeIndex(nodes) {
  return new Map(nodes.map((node, index) => [node.id, index]))
}

export function createLinearWorkflowEdges(nodes = [], idFactory) {
  return nodes.slice(1).map((node, index) => {
    const source = nodes[index]
    return {
      id: idFactory?.(index, source.id, node.id) || `edge-${index}-${source.id}-${node.id}`,
      source: source.id,
      sourcePort: 'output',
      target: node.id,
      targetPort: 'input',
    }
  })
}

export function normalizeWorkflowEdges(edges = [], nodes = [], idFactory) {
  const ids = new Set(nodes.map((node) => node.id))
  const seen = new Set()
  const normalized = []
  for (const [index, edge] of edges.entries()) {
    const source = String(edge?.source || '')
    const target = String(edge?.target || '')
    const sourcePort = SOURCE_PORTS.has(edge?.sourcePort) ? edge.sourcePort : 'output'
    if (!ids.has(source) || !ids.has(target) || source === target) continue
    const key = `${source}:${sourcePort}:${target}`
    if (seen.has(key)) continue
    seen.add(key)
    normalized.push({
      id: String(
        edge?.id || idFactory?.(index, source, target) || `edge-${index}-${source}-${target}`,
      ),
      source,
      sourcePort,
      target,
      targetPort: 'input',
    })
  }
  return normalized
}

export function analyzeWorkflowGraph(nodes = [], edges = []) {
  const activeNodes = nodes.filter((node) => node.enabled !== false)
  const activeIds = new Set(activeNodes.map((node) => node.id))
  const activeEdges = edges.filter(
    (edge) => activeIds.has(edge.source) && activeIds.has(edge.target),
  )
  const incoming = new Map(activeNodes.map((node) => [node.id, []]))
  const outgoing = new Map(activeNodes.map((node) => [node.id, []]))
  const indegree = new Map(activeNodes.map((node) => [node.id, 0]))
  const indexes = nodeIndex(activeNodes)

  for (const edge of activeEdges) {
    incoming.get(edge.target)?.push(edge)
    outgoing.get(edge.source)?.push(edge)
    indegree.set(edge.target, (indegree.get(edge.target) || 0) + 1)
  }

  const ready = activeNodes.filter((node) => indegree.get(node.id) === 0)
  const order = []
  while (ready.length) {
    ready.sort((a, b) => indexes.get(a.id) - indexes.get(b.id))
    const node = ready.shift()
    order.push(node)
    for (const edge of outgoing.get(node.id) || []) {
      const next = (indegree.get(edge.target) || 0) - 1
      indegree.set(edge.target, next)
      if (next === 0) ready.push(activeNodes.find((item) => item.id === edge.target))
    }
  }

  const roots = activeNodes.filter((node) => (incoming.get(node.id) || []).length === 0)
  const preferredRoot = roots.find((node) => node.kind === 'trigger') || roots[0]
  const unconnected = activeNodes.filter(
    (node) => node.id !== preferredRoot?.id && (incoming.get(node.id) || []).length === 0,
  )
  const invalidTriggerTargets = activeNodes.filter(
    (node) => node.kind === 'trigger' && (incoming.get(node.id) || []).length > 0,
  )

  return {
    nodes: activeNodes,
    edges: activeEdges,
    incoming,
    outgoing,
    order,
    roots,
    unconnected,
    invalidTriggerTargets,
    hasCycle: order.length !== activeNodes.length,
  }
}

export function wouldCreateWorkflowCycle(nodes, edges, source, target, sourcePort = 'output') {
  const candidate = [
    ...edges,
    { id: '__candidate__', source, sourcePort, target, targetPort: 'input' },
  ]
  return analyzeWorkflowGraph(nodes, candidate).hasCycle
}
