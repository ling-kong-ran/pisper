// JSON.stringify emits a literal `\uDxxx` escape for a lone surrogate that can
// arise when UTF-16 code-unit slicing (e.g. String.prototype.slice) cuts a
// surrogate pair in half. Browsers tolerate it; Rust's serde_json rejects it and
// tears down the whole TUI stream. Normalise lone surrogates to U+FFFD before
// any response leaves the runtime so both clients always receive strict JSON.
export function replaceLoneSurrogates(value) {
  let index = 0
  while (index < value.length) {
    const code = value.charCodeAt(index)
    if (code >= 0xdc00 && code <= 0xdfff) break // lone low surrogate
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(index + 1)
      if (next >= 0xdc00 && next <= 0xdfff) {
        index += 2 // valid surrogate pair
        continue
      }
      break // lone high surrogate
    }
    index++
  }
  if (index >= value.length) return value
  let result = value.slice(0, index) + '\ufffd'
  index++
  while (index < value.length) {
    const code = value.charCodeAt(index)
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(index + 1)
      if (next >= 0xdc00 && next <= 0xdfff) {
        result += value[index] + value[index + 1]
        index += 2
        continue
      }
      result += '\ufffd'
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      result += '\ufffd'
    } else {
      result += value[index]
    }
    index++
  }
  return result
}

function jsonReplacer(_key, value) {
  return typeof value === 'string' ? replaceLoneSurrogates(value) : value
}

export function json(res, status, body) {
  res.writeHead(status, {
    'Content-Type': 'application/json; charset=utf-8',
    'Cache-Control': 'no-store',
  })
  res.end(JSON.stringify(body, jsonReplacer))
}

export async function bodyJson(req) {
  const chunks = []
  let size = 0
  for await (const chunk of req) {
    size += chunk.length
    if (size > 32_000_000) throw new Error('请求体过大，附件总大小不能超过约 24 MB。')
    chunks.push(chunk)
  }
  if (!chunks.length) return {}
  return JSON.parse(Buffer.concat(chunks).toString('utf8'))
}

export function sseSend(res, event, data) {
  res.write(`event: ${event}\ndata: ${JSON.stringify(data, jsonReplacer)}\n\n`)
}
