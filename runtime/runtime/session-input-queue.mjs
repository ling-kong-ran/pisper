import { AsyncLocalStorage } from 'node:async_hooks'
import { randomUUID } from 'node:crypto'

const inputContext = new AsyncLocalStorage()
const inputMetadata = new WeakMap()
const observedAgents = new WeakSet()
const queueSnapshots = new WeakMap()
let lastRevision = Date.now()

function messageText(message) {
  if (message?.role !== 'user') return ''
  if (typeof message.content === 'string') return message.content
  return (message.content || [])
    .filter((part) => part.type === 'text')
    .map((part) => part.text)
    .join('')
}

// Pi 目前没有单条撤回 API。私有队列访问集中在此处，并由真实 SDK 合同测试约束；
// 必须检查执行队列，不能把已经出队、尚未广播 message_start 的输入误判为可撤回。
function engineQueues(session) {
  const steering = session?.agent?.steeringQueue
  const followUp = session?.agent?.followUpQueue
  if (!Array.isArray(steering?.messages) || !Array.isArray(followUp?.messages)) return null
  return [
    { behavior: 'steer', queue: steering, display: session.getSteeringMessages?.() },
    { behavior: 'followUp', queue: followUp, display: session.getFollowUpMessages?.() },
  ]
}

function metadataFor(message) {
  let metadata = inputMetadata.get(message)
  if (!metadata) {
    metadata = { id: randomUUID() }
    inputMetadata.set(message, metadata)
  }
  return metadata
}

function observeQueuedInputs(session) {
  const agent = session.agent
  if (!agent || observedAgents.has(agent)) return
  for (const behavior of ['steer', 'followUp']) {
    const enqueue = agent[behavior]
    if (typeof enqueue !== 'function') continue
    agent[behavior] = function (message) {
      const context = inputContext.getStore()
      if (context?.session === session && context.behavior === behavior && !context.inputId) {
        const metadata = metadataFor(message)
        metadata.original = context.original
        context.inputId = metadata.id
      }
      return enqueue.call(this, message)
    }
  }
  observedAgents.add(agent)
}

// 使用异步调用上下文关联原始附件，避免并发入队或扩展转换文本时串到另一条消息。
export async function captureQueuedSessionInput(session, behavior, original, prompt) {
  observeQueuedInputs(session)
  const context = { session, behavior, original, inputId: null }
  await inputContext.run(context, prompt)
  return context.inputId
}

export function sessionInputQueueSnapshot(session) {
  if (!session) return { inputs: [], queueRevision: lastRevision }
  const queues = engineQueues(session)
  const inputs = queues
    ? queues.flatMap(({ behavior, queue }) =>
        queue.messages
          .filter((message) => message.role === 'user')
          .map((message) => ({
            id: metadataFor(message).id,
            behavior,
            text: messageText(message),
          })),
      )
    : [
        ...(session.getSteeringMessages?.() || []).map((text) => ({ behavior: 'steer', text })),
        ...(session.getFollowUpMessages?.() || []).map((text) => ({ behavior: 'followUp', text })),
      ]
  const signature = JSON.stringify(inputs)
  let snapshot = queueSnapshots.get(session)
  if (!snapshot || snapshot.signature !== signature) {
    lastRevision = Math.max(lastRevision + 1, Date.now())
    snapshot = { signature, inputs, queueRevision: lastRevision }
    queueSnapshots.set(session, snapshot)
  }
  return { inputs: snapshot.inputs, queueRevision: snapshot.queueRevision }
}

export function sessionInputQueueRevision(session) {
  return sessionInputQueueSnapshot(session).queueRevision
}

export function releaseConsumedSessionInput(message) {
  const metadata = inputMetadata.get(message)
  // 消费后无需再恢复上传附件，避免历史消息对象长期保留原始文档数据。
  if (metadata) delete metadata.original
}

export function withdrawQueuedSessionInput(session, inputId) {
  const queues = engineQueues(session)
  if (!queues || typeof session._emitQueueUpdate !== 'function') {
    throw new Error('当前引擎版本不支持逐条撤回待发送消息。')
  }
  for (const { behavior, queue, display } of queues) {
    const index = queue.messages.findIndex((message) => inputMetadata.get(message)?.id === inputId)
    if (index < 0) continue
    const message = queue.messages[index]
    const text = messageText(message)
    const currentDisplay =
      behavior === 'steer' ? session.getSteeringMessages() : session.getFollowUpMessages()
    if (!Array.isArray(display) || display !== currentDisplay)
      throw new Error('待发送消息队列状态不兼容，未执行撤回。')
    // 展示队列可能还包含已出队的同文输入；从尾部按剩余同文条数定位，保留其消费记账。
    let laterMatches = queue.messages
      .slice(index + 1)
      .filter((item) => messageText(item) === text).length
    let displayIndex = display.length - 1
    while (displayIndex >= 0) {
      if (display[displayIndex] === text && laterMatches-- === 0) break
      displayIndex -= 1
    }
    if (displayIndex < 0) throw new Error('待发送消息队列状态不一致，未执行撤回。')
    const metadata = inputMetadata.get(message)
    queue.messages.splice(index, 1)
    display.splice(displayIndex, 1)
    session._emitQueueUpdate()
    return metadata.original || { text, attachments: [] }
  }
  return null
}
