// Responses 传输兼容：从 delta 还原中转站缺失的工具参数，
// 并在协议终态结束 SDK 读取，避免网关结算后因 HTTP 未关闭而继续等待。
/**
 * @param {string} block
 * @param {Map<string | number, string>} toolArguments
 * @returns {{ block: string, terminal: boolean }}
 */
function normalizeResponsesEventBlock(block, toolArguments) {
  const lines = block.split(/\r\n|\r|\n/)
  const dataLines = []
  const dataIndexes = []
  for (let index = 0; index < lines.length; index += 1) {
    if (!lines[index].startsWith('data:')) continue
    dataIndexes.push(index)
    dataLines.push(lines[index].slice(5).replace(/^ /, ''))
  }
  const unchanged = { block, terminal: false }
  if (!dataLines.length) return unchanged
  const data = dataLines.join('\n')
  if (data.trim() === '[DONE]') return { block, terminal: true }

  /** @type {unknown} */
  let event
  try {
    event = JSON.parse(data)
  } catch {
    return unchanged
  }
  if (!event || typeof event !== 'object' || Array.isArray(event)) return unchanged
  const responseEvent = /** @type {Record<string, unknown>} */ (event)
  if (
    responseEvent.type === 'response.completed' ||
    responseEvent.type === 'response.incomplete' ||
    responseEvent.type === 'response.failed' ||
    responseEvent.type === 'error'
  )
    return { block, terminal: true }

  const outputIndex =
    typeof responseEvent.output_index === 'string' || typeof responseEvent.output_index === 'number'
      ? responseEvent.output_index
      : 'unknown'
  const item = responseEvent.item
  const functionCall =
    item && typeof item === 'object' && !Array.isArray(item)
      ? /** @type {Record<string, unknown>} */ (item)
      : undefined
  if (
    responseEvent.type === 'response.output_item.added' &&
    functionCall?.type === 'function_call'
  ) {
    toolArguments.set(
      outputIndex,
      typeof functionCall.arguments === 'string' ? functionCall.arguments : '',
    )
    return unchanged
  }
  if (responseEvent.type === 'response.function_call_arguments.delta') {
    if (typeof responseEvent.delta === 'string') {
      toolArguments.set(outputIndex, (toolArguments.get(outputIndex) || '') + responseEvent.delta)
    }
    return unchanged
  }
  if (responseEvent.type !== 'response.function_call_arguments.done') return unchanged

  const argumentsText =
    typeof responseEvent.arguments === 'string'
      ? responseEvent.arguments
      : (toolArguments.get(outputIndex) ?? '')
  toolArguments.delete(outputIndex)
  if (typeof responseEvent.arguments === 'string') return unchanged

  responseEvent.arguments = argumentsText
  lines[dataIndexes[0]] = `data: ${JSON.stringify(responseEvent)}`
  for (const index of dataIndexes.slice(1).reverse()) lines.splice(index, 1)
  return { block: lines.join('\n'), terminal: false }
}

// SSE 的空行结束一个事件。逐行识别 CRLF/LF/CR，不能把单个 CRLF
// 回溯成两个换行，否则分块恰好落在 CR/LF 之间时会提前截断终态帧。
/** @param {string} buffer @returns {number} */
function sseEventEnd(buffer) {
  const newline = /\r\n|\r|\n/g
  let lineStart = 0
  let match
  while ((match = newline.exec(buffer))) {
    if (match.index === lineStart) return newline.lastIndex
    if (match[0] === '\r' && newline.lastIndex === buffer.length) return 0
    lineStart = newline.lastIndex
  }
  return 0
}

/** @param {Response} response @returns {Response} */
function normalizeResponsesStream(response) {
  if (
    !response.body ||
    !response.headers.get('content-type')?.toLowerCase().includes('text/event-stream')
  )
    return response

  const decoder = new TextDecoder()
  const encoder = new TextEncoder()
  const toolArguments = new Map()
  let buffer = ''
  const body = response.body.pipeThrough(
    new TransformStream({
      transform(chunk, controller) {
        buffer += decoder.decode(chunk, { stream: true })
        let end
        while ((end = sseEventEnd(buffer))) {
          const result = normalizeResponsesEventBlock(buffer.slice(0, end), toolArguments)
          buffer = buffer.slice(end)
          controller.enqueue(encoder.encode(result.block))
          if (result.terminal) {
            // response.completed 等已经是模型终态；SDK 仍迭代到 HTTP EOF。
            // 关闭投影并取消上游剩余读取，避免网关已结算而 Agent 永远等在 Thinking。
            buffer = ''
            toolArguments.clear()
            controller.terminate()
            return
          }
        }
      },
      flush(controller) {
        buffer += decoder.decode()
        if (buffer)
          controller.enqueue(
            encoder.encode(normalizeResponsesEventBlock(buffer, toolArguments).block),
          )
      },
    }),
  )
  const headers = new Headers(response.headers)
  headers.delete('content-length')
  return new Response(body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  })
}

// 在 SDK 合并默认头之后过滤诊断信息，不改变鉴权、请求体或传输选项。
/**
 * @param {typeof globalThis.fetch} [fetchImpl]
 * @param {{ normalizeResponses?: boolean }} [options]
 * @returns {typeof globalThis.fetch}
 */
export function createOpenAIRequestFetch(fetchImpl = globalThis.fetch, options = {}) {
  return async (input, init) => {
    const headers = new Headers(
      init?.headers ?? (input instanceof Request ? input.headers : undefined),
    )
    for (const name of [...headers.keys()]) {
      if (name.toLowerCase().startsWith('x-stainless-')) headers.delete(name)
    }
    const response = await fetchImpl(input, { ...init, headers })
    return options.normalizeResponses ? normalizeResponsesStream(response) : response
  }
}

/** @param {{ api?: string }} model */
export function usesOpenAISdk(model) {
  return (
    model.api === 'openai-completions' ||
    model.api === 'openai-responses' ||
    model.api === 'azure-openai-responses'
  )
}
