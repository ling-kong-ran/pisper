// 默认不读取时钟也不输出日志；保留阶段字符串参数，兼容现有诊断调用方。
export function createStartupObserver(observer, { now = () => performance.now() } = {}) {
  if (typeof observer !== 'function') return () => {}
  const started = now()
  let previous = started
  return (stage) => {
    const current = now()
    const timing = { elapsedMs: current - started, durationMs: current - previous }
    previous = current
    try {
      observer(stage, timing)
    } catch {
      // 诊断失败不能改变启动结果。
    }
  }
}

export function createStartupLogObserver(environment = process.env, log = console.info) {
  if (environment.PISPER_STARTUP_TRACE !== '1' && environment.PISPER_STARTUP_DIAGNOSTICS !== '1')
    return null
  return (stage, timing) => log(JSON.stringify({ event: 'pisper-startup', stage, ...timing }))
}
