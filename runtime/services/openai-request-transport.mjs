// Responses 中转站有时省略 function_call_arguments.done.arguments；从 delta 还原，
// 避免 Pi 的流解析器对缺失值调用 startsWith，同时保持响应体流式转发。
/**
 * @param {string} block
 * @param {Map<string | number, string>} toolArguments
 * @returns {string}
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
  if (!dataLines.length) return block

  /** @type {unknown} */
  let event
  try {
    event = JSON.parse(dataLines.join('\n'))
  } catch {
    return block
  }
  if (!event || typeof event !== 'object' || Array.isArray(event)) return block
  const responseEvent = /** @type {Record<string, unknown>} */ (event)

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
    return block
  }
  if (responseEvent.type === 'response.function_call_arguments.delta') {
    if (typeof responseEvent.delta === 'string') {
      toolArguments.set(outputIndex, (toolArguments.get(outputIndex) || '') + responseEvent.delta)
    }
    return block
  }
  if (responseEvent.type !== 'response.function_call_arguments.done') return block

  const argumentsText =
    typeof responseEvent.arguments === 'string'
      ? responseEvent.arguments
      : (toolArguments.get(outputIndex) ?? '')
  toolArguments.delete(outputIndex)
  if (typeof responseEvent.arguments === 'string') return block

  responseEvent.arguments = argumentsText
  lines[dataIndexes[0]] = `data: ${JSON.stringify(responseEvent)}`
  for (const index of dataIndexes.slice(1).reverse()) lines.splice(index, 1)
  return lines.join('\n')
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
        let separator
        while ((separator = /(?:\r\n|\r|\n){2}/.exec(buffer))) {
          const block = buffer.slice(0, separator.index)
          buffer = buffer.slice(separator.index + separator[0].length)
          controller.enqueue(
            encoder.encode(normalizeResponsesEventBlock(block, toolArguments) + separator[0]),
          )
        }
      },
      flush(controller) {
        buffer += decoder.decode()
        if (buffer)
          controller.enqueue(encoder.encode(normalizeResponsesEventBlock(buffer, toolArguments)))
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
