import { createHash } from 'node:crypto'

function stableValue(value) {
  if (Array.isArray(value)) return value.map((item) => stableValue(item))
  if (value && typeof value === 'object')
    return Object.fromEntries(
      Object.entries(value)
        .filter(([key]) => key !== 'execute')
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, child]) => [key, stableValue(child)]),
    )
  return value
}

function hash(value) {
  return createHash('sha256').update(JSON.stringify(value)).digest('hex').slice(0, 16)
}

function toolShape(tool) {
  return {
    name: String(tool?.name || ''),
    description: String(tool?.description || ''),
    parameters: stableValue(tool?.parameters || tool?.inputSchema || {}),
  }
}

export function capturePromptCacheShape({ systemPrompt = '', tools = [] } = {}) {
  const normalizedTools = tools
    .map(toolShape)
    .sort((left, right) =>
      `${left.name}\n${left.description}\n${JSON.stringify(left.parameters)}`.localeCompare(
        `${right.name}\n${right.description}\n${JSON.stringify(right.parameters)}`,
      ),
    )
  const systemHash = hash(String(systemPrompt))
  const toolsHash = hash(normalizedTools)
  return {
    systemHash,
    toolsHash,
    prefixHash: hash({ system: String(systemPrompt), tools: normalizedTools }),
    toolCount: normalizedTools.length,
    toolSchemaBytes: Buffer.byteLength(JSON.stringify(normalizedTools), 'utf8'),
    changed: false,
    changeReasons: [],
  }
}

export function comparePromptCacheShapes(previous, current) {
  if (!previous) return { ...current, changed: false, changeReasons: [] }
  const changeReasons = []
  if (previous.systemHash !== current.systemHash) changeReasons.push('system')
  if (previous.toolsHash !== current.toolsHash) changeReasons.push('tools')
  return { ...current, changed: changeReasons.length > 0, changeReasons }
}
