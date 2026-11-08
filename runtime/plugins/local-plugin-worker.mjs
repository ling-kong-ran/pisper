// 本地插件 Worker：第三方插件在隔离的 worker_thread 中执行，
// 插件必须导出 execute 函数；错误经序列化回传给主线程。
import { parentPort, workerData } from 'node:worker_threads'

function serializeError(error) {
  return {
    message: String(error?.message || error || 'Plugin failed.'),
    stack: typeof error?.stack === 'string' ? error.stack : undefined,
  }
}

async function main() {
  try {
    const module = await import(workerData.entryUrl)
    const execute = module.execute || module.default?.execute || module.default
    if (typeof execute !== 'function') {
      throw new Error('Plugin entry must export an execute function.')
    }
    const result = await execute({
      toolName: workerData.toolName,
      arguments: workerData.arguments,
      context: workerData.context,
    })
    parentPort?.postMessage({ type: 'result', result })
  } catch (error) {
    parentPort?.postMessage({ type: 'error', error: serializeError(error) })
  }
}

void main()
