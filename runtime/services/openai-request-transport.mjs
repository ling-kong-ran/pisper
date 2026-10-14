// 在 SDK 合并默认头之后过滤诊断信息，不改变鉴权、请求体或传输选项。
/**
 * @param {typeof globalThis.fetch} [fetchImpl]
 * @returns {typeof globalThis.fetch}
 */
export function createOpenAIRequestFetch(fetchImpl = globalThis.fetch) {
  return (input, init) => {
    const headers = new Headers(
      init?.headers ?? (input instanceof Request ? input.headers : undefined),
    )
    for (const name of [...headers.keys()]) {
      if (name.toLowerCase().startsWith('x-stainless-')) headers.delete(name)
    }
    return fetchImpl(input, { ...init, headers })
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
