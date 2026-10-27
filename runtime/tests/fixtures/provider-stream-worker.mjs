// 双终端回归的独立 Runtime 进程：只使用父测试提供的隔离目录和回环网关。
import { AgentRuntimeService } from '../../runtime/agent-runtime.mjs'

async function runWorker({ workspace, dataDir, name }) {
  const runtime = new AgentRuntimeService({ cwd: workspace, dataDir })
  let result
  try {
    await runtime.init()
    // 只隔离公网目录与记忆检索/提取，保留真实 Pi Agent、HTTP 模型流和会话收尾。
    runtime.modelMetadata.ensure = async () => null
    runtime.memory.relevantContext = async () => ({ text: '', memories: [] })
    runtime.captureConversationMemory = async () => {}
    const session = await runtime.createSession(name, workspace)
    const events = []
    await runtime.streamPrompt({
      sessionId: session.id,
      message: 'Finish the isolated fixture turn.',
      send(event, data) {
        if (event === 'done' || event === 'error')
          events.push({ event, text: data.text, message: data.message })
      },
    })
    result = {
      type: 'finished',
      events,
      streaming: runtime.liveSessions.get(session.id)?.streaming ?? false,
    }
  } catch (error) {
    // 失败只报告稳定类别，不把配置、凭据或临时目录写入测试输出。
    result = { type: 'failed', code: String(error?.code || error?.name || 'worker_failed') }
    process.exitCode = 1
  } finally {
    await runtime.dispose()
  }
  await new Promise((resolve) => process.send(result, resolve))
  process.disconnect()
}

process.once('message', (input) => {
  void runWorker(input).catch(() => {
    process.exitCode = 1
    if (process.connected) process.disconnect()
  })
})
