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

export async function bodyBuffer(req, maxBytes = 32_000_000) {
  const chunks = []
  let size = 0
  for await (const chunk of req) {
    size += chunk.length
    if (size > maxBytes) throw new Error('请求体过大。')
    chunks.push(chunk)
  }
  return Buffer.concat(chunks)
}

export async function bodyJson(req) {
  const buffer = await bodyBuffer(req)
  if (!buffer.length) return {}
  return JSON.parse(buffer.toString('utf8'))
}

// 序列化一帧 SSE 的 data 负载（含孤立代理项清洗）。
// 导出给调用方复用：同一帧既要写入 run 环形缓冲又要写出连接时，
// 只序列化一次，两处共享同一 payload 字符串。
export function serializeSsePayload(data) {
  return JSON.stringify(data, jsonReplacer) || ''
}

// 发送一帧 SSE：id 非空时写入标准 `id:` 行（run 游标），
// 旧客户端会忽略未知字段，保持前向兼容。
// payload 允许传入预序列化结果，避免热路径上重复 JSON.stringify。
// 返回值是 res.write() 的背压信号：false 仅表示本次写入后缓冲越过了
// Node 的 ~16KB 高水位——单帧稍大就会命中，并不代表客户端停止消费，
// 调用方不应据此断连（需要区分慢客户端时请用 sseSendGuarded）。
export function sseSend(res, event, data, id = null, payload = null) {
  const idLine = id == null ? '' : `id: ${id}\n`
  const body = payload ?? serializeSsePayload(data)
  return res.write(`${idLine}event: ${event}\ndata: ${body}\n\n`) !== false
}

// SSE 心跳间隔：空闲期每 15s 写一帧注释行（`:\n\n`）。
// 长思考/长 bash 期间没有业务帧，移动网络与反常会静默断开空闲连接；
// 两端的 SSE 解析器都会跳过 `:` 注释行，协议上零成本。
export const SSE_HEARTBEAT_INTERVAL_MS = 15_000

const sseHeartbeatTimers = new WeakMap()

export function stopSseHeartbeat(res) {
  const timer = sseHeartbeatTimers.get(res)
  if (timer) clearInterval(timer)
  sseHeartbeatTimers.delete(res)
}

// 为一条 SSE 响应启动心跳；重复调用幂等，连接关闭时自动停止。
// 心跳不经过 sseSendGuarded：它不该重置慢客户端的 stall 判定。
export function startSseHeartbeat(res) {
  if (sseHeartbeatTimers.has(res)) return
  const timer = setInterval(() => {
    if (res.destroyed || res.writableEnded) {
      stopSseHeartbeat(res)
      return
    }
    res.write(':\n\n')
  }, SSE_HEARTBEAT_INTERVAL_MS)
  timer.unref?.()
  sseHeartbeatTimers.set(res, timer)
  res.once?.('close', () => stopSseHeartbeat(res))
}

// drain 迟迟不来的判死时限：瞬时背压（write 返回 false）只表示越过 ~16KB 高水位，
// 健康客户端由内核立即排空；数据持续排不出去才说明客户端真的停止消费，
// 此时销毁连接让其凭 run 游标重挂补发，事件不丢。
export const SSE_STALL_TIMEOUT_MS = 30_000

const sseStallStates = new WeakMap()

function clearSseStallState(res) {
  const state = sseStallStates.get(res)
  if (!state) return
  clearTimeout(state.timer)
  res.removeListener?.('drain', state.onDrain)
  res.removeListener?.('close', state.onClose)
  sseStallStates.delete(res)
}

// 带慢客户端护栏的 SSE 发送：瞬时背压不处理（等 drain 自然恢复），
// 只有超过 stall 时限仍未 drain 且仍有积压时才销毁连接——被断开的客户端
// 可凭 run 游标经 GET /api/runs/:id/events?after= 重挂补发，事件不会丢。
export function sseSendGuarded(res, event, data, id = null, payload = null) {
  if (res.destroyed || res.writableEnded) return false
  const accepted = sseSend(res, event, data, id, payload)
  if (accepted) {
    clearSseStallState(res)
    return true
  }
  if (!sseStallStates.has(res)) {
    const onDrain = () => clearSseStallState(res)
    const onClose = () => clearSseStallState(res)
    const timer = setTimeout(() => {
      clearSseStallState(res)
      // 超时后仍有数据排不出去才判死；期间客户端恢复读取（drain）则已解除。
      if (!res.destroyed && !res.writableEnded && (Number(res.writableLength) || 0) > 0)
        res.destroy?.()
    }, SSE_STALL_TIMEOUT_MS)
    timer.unref?.()
    sseStallStates.set(res, { timer, onDrain, onClose })
    res.once?.('drain', onDrain)
    res.once?.('close', onClose)
  }
  return true
}
