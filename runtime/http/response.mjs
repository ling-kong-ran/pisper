// JSON.stringify 对孤立代理项（lone surrogate）会输出字面量 \uDxxx 转义。
// 浏览器容忍它，但 Rust 的 serde_json 会拒绝并摧毁整个 TUI 流。
// 因此在任何响应离开运行时之前，把所有孤立代理项归一化为 U+FFFD，
// 保证两个客户端（Web/TUI）都收到严格合法的 JSON。
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

// 统一 JSON 响应：序列化时清洗字符串中的孤立代理项。
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
