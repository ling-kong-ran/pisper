// 插队（立即发送）：运行中打断“当前这一步”——正在生成的模型回复停在已经写出的位置，正在执行的工具中止——
// 但运行本身不结束：插队消息已经在 Pi 的 steering 队列里，Pi 的循环在这一步收尾时读到它，马上开始下一步。
// 排队（steer）则等这一步自己走完；两者只差“要不要打断”。
//
// Pi 没有“只中止当前一步”的接口：中止信号是整轮运行共用的，一旦触发整轮就结束，排着的消息也就没人处理。
// 所以这里给每一次模型请求、每一次工具执行各套一个子信号，打断时只中止子信号：
// - 模型请求：被打断的回复改记为正常结束（stopReason 'stop'），只保留已写出的正文；没写完的工具调用与
//   思考片段丢掉（重放半截工具调用或思考会让一些接口报错）。正文为空时各家接口都会跳过这条消息。
// - 工具：中止信号交给工具自己收尾，结果照常记为一条工具结果。
// 私有接入点（agent.runWithLifecycle、agent.streamFunction、agent.beforeToolCall）由真实 SDK 合同测试约束。
const STEP_INTERRUPT_PATCH = Symbol('pisper.step-interrupt')

function linkedStep(parent) {
  const controller = new AbortController()
  const step = { signal: controller.signal, interrupted: false }
  const forward = () => controller.abort(parent?.reason)
  if (parent?.aborted) forward()
  else parent?.addEventListener?.('abort', forward, { once: true })
  step.interrupt = () => {
    if (controller.signal.aborted) return
    step.interrupted = true
    controller.abort(new Error('interrupted'))
  }
  step.dispose = () => parent?.removeEventListener?.('abort', forward)
  return step
}

// 被打断的回复：只留正文，按正常结束记账。
function settledMessage(message) {
  const content = (Array.isArray(message?.content) ? message.content : []).filter(
    (part) => part?.type === 'text' && typeof part.text === 'string' && part.text.length > 0,
  )
  const { errorMessage: _errorMessage, ...rest } = message || {}
  return { ...rest, role: 'assistant', content, stopReason: 'stop' }
}

function interruptibleResponse(response, step, release) {
  let settled = null
  return {
    async *[Symbol.asyncIterator]() {
      let partial = null
      try {
        for await (const event of response) {
          if (event?.partial) partial = event.partial
          if (step.interrupted && (event?.type === 'error' || event?.type === 'done')) {
            settled = settledMessage(event.error || event.message || partial)
            yield { type: 'done', reason: 'stop', message: settled }
            return
          }
          yield event
        }
      } finally {
        release()
      }
    },
    async result() {
      return settled || response.result()
    },
  }
}

/**
 * @param {any} session Pi AgentSession
 */
export function installStepInterrupt(session) {
  const agent = session?.agent
  if (
    !agent ||
    session[STEP_INTERRUPT_PATCH] ||
    typeof agent.runWithLifecycle !== 'function' ||
    typeof agent.streamFunction !== 'function'
  )
    return session
  const active = new Set()
  const track = (parent) => {
    const step = linkedStep(parent)
    active.add(step)
    return {
      step,
      release: () => {
        active.delete(step)
        step.dispose()
      },
    }
  }

  // 每次运行开始时 Pi 同步读取一次 streamFunction 交给循环：这里只在那一刻换成可打断的版本，
  // 随即换回原函数，压缩摘要等其他调用方看到的仍是 Pi 原来的函数（Pi 用它的身份决定取凭据的方式）。
  const runWithLifecycle = agent.runWithLifecycle
  agent.runWithLifecycle = function (run) {
    return runWithLifecycle.call(this, (signal) => {
      const stream = this.streamFunction
      this.streamFunction = async (model, context, options = {}) => {
        const { step, release } = track(options.signal)
        let response
        try {
          response = await stream(model, context, { ...options, signal: step.signal })
        } catch (error) {
          release()
          throw error
        }
        return interruptibleResponse(response, step, release)
      }
      try {
        return run(signal)
      } finally {
        this.streamFunction = stream
      }
    })
  }

  // 工具：执行前把本会话的工具实例包一层，执行时用子信号。共享给其他会话的工具实例不包（打断会串到别的会话），
  // 那种工具照常执行完，插队消息在它结束后立刻送达。
  const wrapped = new WeakSet()
  const foreign = new WeakSet()
  const previous = agent.beforeToolCall
  agent.beforeToolCall = async (context, signal) => {
    const decision = await previous?.(context, signal)
    const tool = context?.context?.tools?.find((item) => item?.name === context?.toolCall?.name)
    if (tool && typeof tool.execute === 'function' && !wrapped.has(tool) && !foreign.has(tool)) {
      if (tool[STEP_INTERRUPT_PATCH] && tool[STEP_INTERRUPT_PATCH] !== session) foreign.add(tool)
      else {
        const execute = tool.execute
        tool[STEP_INTERRUPT_PATCH] = session
        tool.execute = async function (toolCallId, args, toolSignal, ...rest) {
          const { step, release } = track(toolSignal)
          try {
            return await execute.call(this, toolCallId, args, step.signal, ...rest)
          } finally {
            release()
          }
        }
        wrapped.add(tool)
      }
    }
    return decision
  }

  session[STEP_INTERRUPT_PATCH] = {
    // 打断当前这一步；返回是否真的打断了什么（模型请求或工具执行正在进行）。
    interrupt() {
      let interrupted = false
      for (const step of active) {
        if (step.signal.aborted) continue
        step.interrupt()
        interrupted = true
      }
      return interrupted
    },
  }
  return session
}

export function interruptCurrentStep(session) {
  return session?.[STEP_INTERRUPT_PATCH]?.interrupt?.() ?? false
}
