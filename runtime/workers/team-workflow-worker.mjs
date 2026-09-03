import { parentPort, workerData } from 'node:worker_threads'
import { Script, createContext } from 'node:vm'

if (!parentPort) throw new Error('Team workflow worker requires a parent port.')

const pendingAgents = new Map()
let nextAgentRequestId = 1
let communicationClosed = false
const WORKFLOW_COMMUNICATION_ERROR_PREFIX = 'Team workflow communication stream interrupted'

function communicationError(message) {
  const error = new Error(`${WORKFLOW_COMMUNICATION_ERROR_PREFIX}: ${message}`)
  error.code = 'TEAM_WORKFLOW_COMMUNICATION_INTERRUPTED'
  return error
}

function rejectPendingAgents(message) {
  if (communicationClosed) return
  communicationClosed = true
  const error = communicationError(message)
  for (const { reject } of pendingAgents.values()) reject(error)
  pendingAgents.clear()
}

parentPort.on('message', (message) => {
  if (message?.type !== 'agent_response') return
  const pending = pendingAgents.get(message.id)
  // 延迟到达的响应属于已结束的请求，不能重新唤醒脚本或覆盖新的请求状态。
  if (!pending) return
  pendingAgents.delete(message.id)
  if (message.error) {
    const error = new Error(String(message.error.message || message.error))
    if (message.error.code) error.code = String(message.error.code)
    pending.reject(error)
  } else pending.resolve(String(message.resultJson || 'null'))
})
parentPort.once('close', () => {
  rejectPendingAgents('the parent Agent runtime closed the bridge')
  clearInterval(heartbeat)
})

function requestAgent(payload) {
  return new Promise((resolve, reject) => {
    if (communicationClosed) {
      reject(communicationError('the parent Agent runtime is unavailable'))
      return
    }
    const id = nextAgentRequestId++
    pendingAgents.set(id, { resolve, reject })
    try {
      parentPort.postMessage({ type: 'agent_request', id, payload: String(payload || '{}') })
    } catch (error) {
      pendingAgents.delete(id)
      rejectPendingAgents(error?.message || 'the Agent request could not be sent')
      reject(communicationError(error?.message || 'the Agent request could not be sent'))
    }
  })
}

Object.setPrototypeOf(requestAgent, null)

const heartbeat = setInterval(() => {
  if (communicationClosed) {
    clearInterval(heartbeat)
    return
  }
  try {
    parentPort.postMessage({ type: 'heartbeat' })
  } catch (error) {
    rejectPendingAgents(error?.message || 'the heartbeat bridge is unavailable')
    clearInterval(heartbeat)
  }
}, 250)
heartbeat.unref?.()

try {
  const context = createContext(
    {
      __pisperAgentBridge: requestAgent,
      __pisperArgsJson: String(workerData.argsJson || ''),
    },
    { codeGeneration: { strings: false, wasm: false } },
  )
  const script = new Script(
    `(async (__pisperApi, __pisperArgs) => {
  // 只把脚本需要的窄 API 放入工作流函数作用域，避免脚本通过闭包参数接触主线程桥接函数。
  delete globalThis.__pisperAgentBridge
  delete globalThis.__pisperArgsJson
  const { agent, parallel, pipeline, phase, log, logs } = __pisperApi
  const args = __pisperArgs
  try {
    const result = await (async () => {
${workerData.body}
    })()
    return { __pisperWorkflowResult: result, logs }
  } catch (error) {
    return { __pisperWorkflowError: String(error?.message || error), logs }
  }
})(((bridge) => {
  let currentPhase = ''
  const agent = async (prompt, options = {}) => {
    try {
      return JSON.parse(await bridge(JSON.stringify({
        prompt,
        options: { ...options, __pisperPhase: currentPhase },
      })))
    } catch (error) {
      throw new Error(String(error?.message || error))
    }
  }
  const parallel = async (items) => {
    if (!Array.isArray(items)) throw new Error('Workflow fan-out expects an array.')
    if (items.length > ${Number(workerData.maxAgents)})
      throw new Error('A workflow fan-out cannot contain more than ${Number(workerData.maxAgents)} items.')
    return Promise.all(items.map((item) => (typeof item === 'function' ? item() : item)))
  }
  const pipeline = async (items, worker) => {
    if (!Array.isArray(items)) throw new Error('Workflow pipeline expects an array.')
    if (typeof worker !== 'function') throw new Error('Workflow pipeline requires a worker function.')
    return parallel(items.map((item) => () => worker(item)))
  }
  const phase = (title) => {
    currentPhase = String(title || '').trim().slice(0, 80)
    return currentPhase
  }
  const logs = []
  const log = (message) => {
    const value = String(message || '').trim()
    if (value && logs.length < 256) logs.push(value.slice(0, 1000))
    return ''
  }
  return Object.freeze({ agent, parallel, pipeline, phase, log, logs })
})(__pisperAgentBridge), __pisperArgsJson ? JSON.parse(__pisperArgsJson) : undefined)`,
    { filename: String(workerData.path || 'team-workflow.js') },
  )
  const rawResult = await script.runInContext(context, {
    timeout: Number(workerData.vmTimeoutMs),
  })
  if (rawResult?.__pisperWorkflowError) throw new Error(rawResult.__pisperWorkflowError)
  const serializedResult = JSON.stringify(rawResult?.__pisperWorkflowResult) || 'null'
  if (Buffer.byteLength(serializedResult, 'utf8') > Number(workerData.maxResultBytes))
    throw new Error(`Workflow results are limited to ${Number(workerData.maxResultBytes)} bytes.`)
  parentPort.postMessage({
    type: 'result',
    resultJson: serializedResult,
    logs: Array.isArray(rawResult?.logs) ? rawResult.logs : [],
  })
} catch (error) {
  parentPort.postMessage({
    type: 'error',
    error: error instanceof Error ? error.message : String(error),
  })
} finally {
  clearInterval(heartbeat)
}
